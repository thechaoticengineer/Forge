# Scenarios

**Status: planned; the engine does not implement any of this yet.**

Acceptance scenarios below use stable IDs (S1, S2, ...), assigned once and never renumbered or reused. This file currently covers milestone M1 only (format and discovery); scenarios for M2-M5 are to be written before those milestones are planned (see `milestones.md`).

## S1: A valid feature folder is discovered

- Given: `docs/features/<slug>/` exists with all required files (`README.md`, `scenarios.md`, `decisions.md`, `milestones.md`)
- When: the engine scans `docs/features/` for feature folders
- Then: the folder is discovered and reported as a valid feature

## S2: Folders starting with an underscore are ignored

- Given: `docs/features/_template/` (or any other folder whose name starts with `_`) exists alongside valid feature folders
- When: the engine scans `docs/features/` for feature folders
- Then: the underscore-prefixed folder is not reported as a feature

## S3: A folder missing a required file is reported invalid

- Given: a feature folder under `docs/features/<slug>/` is missing one of its required files
- When: the engine scans `docs/features/` for feature folders
- Then: the folder is reported as invalid, with a reason that identifies the missing file

## S4: A scenarios.md with duplicate or malformed IDs is reported invalid

- Given: a feature's `scenarios.md` contains two scenarios sharing the same ID, or an ID that does not follow the stable `S<number>` form
- When: the engine validates that feature's folder
- Then: the feature is reported as invalid, with a reason that identifies the offending scenario ID

## S5: A milestones.md referencing an unknown scenario ID is reported invalid

- Given: a feature's `milestones.md` lists a scenario ID that is not defined in that feature's `scenarios.md`
- When: the engine validates that feature's folder
- Then: the feature is reported as invalid, with a reason that identifies the unknown scenario ID

## S6: The read-only API lists features without modifying files

- Given: `docs/features/` contains a mix of valid and invalid feature folders
- When: a client requests the list of features from the engine's read-only API
- Then: the response identifies each feature by slug, title and validation status, and no file under `docs/features/` is created, modified or removed as a result

## S7: The panel lists discovered features with their status

- Given: the engine has discovered one or more feature folders
- When: the user opens the panel's feature list
- Then: each discovered feature is shown with an indication of its validation status

## S8: Opening a feature in nvim from the panel

- Given: the panel's feature list is showing a discovered feature
- When: the user chooses the "open in nvim" action for that feature
- Then: that feature's `README.md` is opened in nvim

## S9: The existing goal/discussion/queue flow is unaffected without feature specs

- Given: `docs/features/` is absent from the repository, or present but empty
- When: the user drives the existing goal, optional discussion, planning, staged execution and queue flow
- Then: that flow behaves exactly as it does today, with no change caused by the absence or emptiness of `docs/features/`
