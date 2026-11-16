# MCP connectors and macros: plan

Status file for the dev-mode MCP feature. Update the checkboxes at the end of every unit of work. Deviations from PLAN.md go in `docs/decisions.md`.

## Decisions (agreed with the user, 2026-11-15)

- Libertum acts as **both an MCP server and an MCP client**.
- Server mode is **read and write**. Edits are gated (see Safety).
- Client mode is a **manual tool runner**: pick a server's tool, fill in arguments, run, show or insert the result. No embedded AI agent.
- Macros come in two kinds, in phases: **declarative TOML recipes** first, then **Koto scripts** (was Rhai; see decisions.md) as a `script` step.
- Everything sits behind **Dev mode**. Dev mode is off by default. The MCP switch is unavailable until dev mode is on. Turning dev mode off turns MCP off and stops all listeners and child processes.

## Safety rules

- The server listens on `127.0.0.1` only and requires a random bearer token.
- The server is read-only until the **Allow edits** switch is on (off by default).
- Each client's first edit asks for approval: Allow for this session, Always allow, or Deny.
- Every edit goes through `Operation`, so each is one undo step; a macro run is one undo group.
- A stdio client server runs a program: show the exact command and ask before the first launch.
- Server and macro work never runs on the keystroke path.
- Koto scripts get a 2 second time limit and no file, program or network access; the only way out of a recipe is a tool of the connected MCP program.

## Dependencies (licences checked 2026-11-15, log in decisions.md when added)

- `rmcp` 3.10.0, Apache-2.0, Rust 1.88 or later. Official Rust MCP SDK, client and server. Needs tokio: check the Iced 0.14 runtime before adding one. Read the pinned source before using its API (rule 1).
- `koto` 0.16.1, MIT (replaces `rhai`, whose `smartstring` dependency is MPL-2.0). Phase 6 only.

## Layout

- New crate `libertum-mcp`: server, client, macro engine. No UI dependency.
- `libertum-app`: thin layer (Settings section, dev panel, approval prompt). User-facing text in `strings.rs`, colours in `theme.rs`.

## Phases

Each phase is done only when fmt is clean, clippy passes with `-D warnings` (with and without `--all-features`), all tests pass, and the macOS compile check passes. Update `docs/STATUS.md` after each.

- [x] **Phase 1: Dev mode gate (done 2026-11-16).** The MCP checkbox is not drawn at all while dev mode is off, and a settings file that sets `mcp_enabled` without `dev_mode` is corrected on load. `Settings::mcp_active()` is the single check later phases must use; it carries a `cfg_attr(not(test), expect(dead_code))` to remove when phase 2 first calls it. Not checked: a screenshot of the Developer section in both themes. `Settings.dev_mode` (off by default) and `Settings.mcp_enabled` (off by default, only settable while dev mode is on). Turning dev mode off clears `mcp_enabled`. Settings window gets a Developer section with both switches. Strings in `strings.rs`. Tests for the dependency between the two switches and persistence in `settings.toml`.
- [x] **Phase 2: Server, read-only (done 2026-11-16).** Verified over the real protocol (initialize, session, tools/list, tools/call, tool error vs protocol error), the 401 and DNS-rebinding refusals, and the app path (enabling starts a loopback listener, a client question is answered from the open document, disabling closes the port). Not done: a trial with a real MCP client such as Claude Desktop; a screenshot of the address and token in Settings; and the document id is per open document, with one document per window. Tools: `list_documents`, `read_text`, `get_outline`, `get_styles`, `get_selection`, `search_text`. Limits: 200 paragraphs, 200,000 bytes, 200 matches, 1,000-character query. Original scope:  `libertum-mcp` crate. Streamable HTTP on loopback with a bearer token. Tools: list open documents, read text, headings, styles, selection, search. A dev panel shows the address and token, with a regenerate button.
- [x] **Phase 3: Server, write (done 2026-11-16).** Allow edits switch, per-client approval prompt (this session, always, deny), `insert_text`, `replace_range`, `format_range` and `set_paragraph_style`, each one undo step. No new `Operation` kinds were needed, so the operation proptest is unchanged; the editor tests cover undo exactness, no coalescing with typing, selection and pending-format preservation and bad ranges. Not done: a trial with a real MCP client; screenshots of the prompt in both themes. Trust is keyed by the client's self-declared name.
- [x] **Phase 4: Client and tool runner (done 2026-11-16, stdio and HTTP).** Save programs, approve each exact command line, connect, list tools, fill a form built from the tool schema, run, then copy the result or insert it at the cursor (one undo step). HTTP servers work too (reqwest with native-tls; https anywhere, plain http only to localhost). An optional bearer token can be saved per web address (2026-11-16). **Not done:** other headers, and sign-in flows (OAuth). The test target is a fixture server (`crates/libertum-mcp/examples/stdio_fixture.rs`, over stdio or with `--http`).
- [x] **Phase 5: Recipe macros (done 2026-11-16).** TOML steps (`get_selection`, `call_tool`, `insert`, `replace_selection`) with `{{variable}}` arguments, in `macros/*.toml` in the data folder (or `LIBERTUM_MACROS`), run from a Macros section of the tool runner window, one undo step per run (new `History::merge_last`). A recipe names one saved program, which must already be connected. Format and safety rules: `docs/STATUS.md`.
- [x] **Phase 6: Koto scripts (done 2026-11-16).** A `script` step (`code`, `save_as`) that turns the recipe's values into one new value, 2 second limit, 512 MB default per-script memory cap and a 1 GB shared cap (both adjustable in Settings > Developer; worker thread plus counting allocator), no host functions: document and tool access stay with the other step kinds.

## Open items

- ~~Which async runtime Iced 0.14 already provides~~ Answered: none (thread-pool executor), so `libertum-mcp` owns a one-thread tokio runtime.
- Whether a stdio bridge is needed for clients that cannot speak HTTP.
- Whether `rmcp` 3.10.0's server transport fits an Iced app's event loop without blocking it.
