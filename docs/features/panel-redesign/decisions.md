# Decisions

## D1: Top tabs rather than a side rail

Decision: Views are switched with a horizontal tab bar under the header.

Reason: The window is 760 px wide by default. A side rail would take width from stage titles and output lines, which already elide. Chosen by the user on 2026-09-18.

## D2: Split Panel.qml into view files

Decision: Each tab view and the stage detail page get their own QML file. `Panel.qml` keeps engine state, polling, API actions and navigation.

Reason: At 3,127 lines, `Panel.qml` is hard for agents and humans to change safely. A redesign that touches every section is the cheapest moment to split it. Chosen by the user on 2026-09-18.

## D3: Move, never remove

Decision: Every action, indicator and shortcut that exists today stays reachable. The redesign only changes where each one lives.

Reason: The goal is compactness and navigation, not a feature cut. Removing something would need its own decision.

## D4: Source-inspection tests follow the code

Decision: The `tests/panel_*.test.mjs` suites that slice `Panel.qml` source text are updated to read the files the code moves to. Their assertions keep their meaning. No test is deleted or weakened to make the split pass.

Reason: The tests protect behavior, not a file layout. Pointing them at the new files keeps that protection.

## D5: Views are StackView-friendly pages, not reloaded components

Decision: Tab views are persistent instances that are shown and hidden, like the existing discussion page, not `Loader`s that rebuild on every switch. Stage detail and the other drill-down pages are pushed onto the existing `panelStack`.

Reason: The panel preserves ListView reading positions and drafts across polls today (see the comments around `stateRequestSerial` and `discussionView`). Rebuilding views on tab switch would lose them.

## D6: Keyboard: brackets and `g` prefixes for views

Decision: `[` / `]` cycle tabs on tab views; on the stage detail page they move to the previous / next stage instead. `g o|p|a|r|f|q|s` jump to a tab from anywhere. Single-letter view keys are not used.

Reason: Single letters such as `p`, `a`, `r` and `f` already trigger actions. The free `g` prefix follows the existing vim-style `gg`.
