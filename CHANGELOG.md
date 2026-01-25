# Changelog

## [4.0.0] - 2026-01-25

# Release Notes

Release v4.0.0  
Range: v3.0.39..HEAD

## Features
- Major frontend refactor: extracted folder, file, sorting, filtering, and tooltip logic into reusable Svelte components.
- Updated frontend to support new backend data formats.
- Added TLS support.
- dusum enhancements: added file size, disk size, and linked size reporting.
- duapi updated to support the new dusum format.
- Added improved shutdown handling to prevent hangs on large file systems.
- Added verbosity controls and later simplified verbosity options.
- Added benchmarking support, including --quiet mode and helper git scripts.
- Added extensive test coverage across multiple areas.
- Added Claude Code guidance documentation for repository layout, Rust backend, and browser frontend.

## Improvements
- Restructured Rust utilities and binaries into module-based directories, replacing monolithic source files.
- Code reorganization in utilities for better maintainability.
- Updated documentation to remove outdated plans and reflect current componentized frontend and file structure.
- Updated vendor.sh cleanup behavior.
- Rolled back to a faster dependency/version after performance regression.
- Improved CI reliability, including fixes to make npm install work.
- General frontend updates and polish aligned with new data structures.

## Fixes
- Fixed duzip to correctly support non-UTF-8 data.
- Fixed CI action version issues.
- Fixed Linux-specific test failures.
- Fixed Linux benchmark issues.
- Removed deprecated push_comma usage.
- Multiple test fixes and stability improvements across the codebase.

## Breaking Changes
- Removed desktop support.
- Backend data format changes (dusum and duapi) require compatible frontend versions.
- Significant Rust codebase restructuring may impact downstream integrations relying on internal module paths.
- Frontend refactor may affect custom extensions or forks relying on previous component structure.

## Infrastructure
- Numerous version bumps throughout the 3.0.x series leading up to 4.0.0.
- Continuous integration fixes and maintenance.
- Repository merges and housekeeping changes to keep master in sync.

