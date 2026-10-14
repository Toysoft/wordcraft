# Multiple desktop documents

Desktop WordCraft keeps one document per process. When a document is already open, File › New
(Ctrl/Command+N), templates and File › Open create another independent window. Recent files,
document drops and Finder/Dock Open events take the same path. The current document, selection,
undo history and unsaved work stay in their window, without a Save Changes prompt.

The first choice on the Start screen opens in that window. Command-line launches with several
files open the first locally and each additional file in another process, even if the first
file fails or needs a password. `wordcraft --template report` starts a report without showing
the Start screen; `--template blank` starts a blank document. `--` ends options, allowing names
such as `--sample` to be opened as files. Spawn failures are reported without replacing work.

Each window has its own close/save prompt and recovery record. Closing the original window
does not close the windows it launched. On macOS, Quit applies to that process, not every
WordCraft process. Use the OS window switcher to move between documents.

## Boundaries

- This does not implement View › New Window as a synchronized view of the same document,
  Split, Switch Windows, Arrange All or Side by Side. The cross-process window coordination
  work in PR #331 is separate.
- Opening the same disk file twice creates independent copies in memory, with no live editing
  synchronization or save conflict detection. Saving both can overwrite the earlier save.
- Preferences and recent files still share the profile; the last window to save preferences
  wins. Each write replaces the preferences file atomically, so another window cannot read a
  partly written JSON file. Logs still use the existing shared rotation scheme.
- A child inherits the profile and rendering environment, but not `WORDCRAFT_CONTROL_PORT`.
  Control servers remain opt-in per process. Run separately with distinct `--control` ports
  when automation needs to address several windows.
- Only user commands (`WordApp::run`) launch windows. Scripts, CLI/MCP and the control channel
  (`WordApp::execute`) still replace the addressed session's document. Hosts without the new
  service, including the browser and headless tests, retain the existing Save Changes flow.
- In-memory uploads and password retries stay local. Passwords and document bytes never travel
  through process arguments; an encrypted disk document asks for its password in its new window.

## Validation

Regression tests cover preserving unsaved text and undo, the first Start screen choice, scripts,
launch errors, template forwarding, option-like/Unicode paths and control-port isolation. The
desktop launcher retains and polls child handles so finished children can be reaped without
blocking the UI; dropping it leaves live children running.

Before release, exercise New/Open, templates, multi-file launches, Finder multi-selection,
encrypted documents, failed opens, editing/saving both documents and closing windows in both
orders on macOS, Windows and Linux (including AppImage and Flatpak). These platform checks are
required in addition to the automated suite; automated tests alone do not verify native window
focus, packaging or OS event routing.
