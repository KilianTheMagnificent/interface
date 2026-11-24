# Interface Update Log

Complete history of production releases (`v0.3.x`) and Dev landings, including notes on what each update is, how it is verified, and when it was completed. Maintained automatically by `cargo xtask closeout`, `cargo xtask promote`, and `cargo xtask update-log`.

- **Production releases:** 115 (`v0.3.46` – `v0.3.182`)
- **Dev landings:** 635 (`0` unreleased on Dev)

## Unreleased on Dev

_None — all Dev landings have shipped in a production release._

## Production Releases

### v0.3.182 — 2026-11-24T17:22:28-06:00 (`e62052f`)

- **[internal]** `53513d2` · 2026-11-24T17:19:56-06:00 · **Scrub personal project names from test and preview fixtures** (`docs/scrub-public-fixtures-20261124`)
- **[fix]** `268ff7c` · 2026-11-24T09:36:16-06:00 · **No false Codex error without Codex; stuck terminals recover while a VM or Internet Sharing is on** (`fix/codex-absent-listing`)
  - Notes: On a Mac without Codex the first screen shows no Codex error; Dev still lists your Codex sessions as before.
  - Pins: `crates/engine-codex/src/adapter_tests.rs::listing_with_no_codex_installed_is_unavailable_not_a_failed_refresh`, `crates/interface-terminal/src/embedded/orphan.rs::a_row_owned_by_nobody_does_not_fail_the_listing`
- **[internal]** `b1fa93d` · 2026-11-23T18:38:01-06:00 · **Public source: README for strangers, MIT licence, one-line installer, export tool** (`docs/public-source-20261123`)
  - Notes: Record the pinned test whose sample path changed
- **[internal]** `37fdc6e` · 2026-11-23T18:34:54-06:00 · **Update rustls-webpki and url past their published advisories** (`fix/dependency-advisories-20261123`)
- **[fix]** `7ee05f5` · 2026-11-23T18:32:01-06:00 · **Browser pane: safe popup windows, quarantined downloads, ask before no-click downloads** (`fix/browser-pane-popups-downloads-20261123`)
  - Notes: In a Browser tab a site's popup always has a title bar and close button; its downloads get the macOS internet-download check; self-started ones ask.
  - Pins: `electron/src/main/browser-pane.test.ts::builds a page's popup window itself: framed, closable, movable, opaque, focusable and never on top`, `electron/src/main/browser-pane.test.ts::keeps a popup window on its screen, no larger than it and never too small to show its site`, `electron/src/main/browser-pane.test.ts::flags a finished download for Gatekeeper before calling it saved, and fails and removes one it cannot flag`, `electron/src/main/browser-pane.test.ts::asks before saving a download the page started without a click, and never for the owner's own`
- **[fix]** `e0b22d4` · 2026-11-23T18:28:53-06:00 · **Team tools answer only their own thread's agent process** (`fix/team-socket-caller-20261123`)
  - Notes: Nothing looks different; agents in threads still start and read other threads, but another agent's command can no longer use a thread's tools.
  - Pins: `apps/interface-host/src/threads_team_tests.rs::a_threads_team_tools_answer_only_its_own_provider_and_the_processes_it_started`, `apps/interface-host/src/team_tools.rs::a_team_socket_answers_only_its_own_provider_and_the_processes_under_it`, `crates/interface-terminal/src/proc_ancestry.rs::a_read_listing_finds_this_test_process_above_a_child_it_spawned`
- **[fix]** `2221a26` · 2026-11-23T18:25:42-06:00 · **Interface's own git calls never run programs a project's .git settings name** (`fix/filter-drivers-20261123`)
  - Notes: Nothing looks different; a downloaded project whose .git names its own filter or ssh program no longer runs it on Review or a checkpoint.
  - Pins: `crates/interface-review/tests/suite/repository_programs.rs::review_never_runs_a_filter_the_repository_defines`, `crates/interface-review/tests/suite/repository_programs.rs::thread_checkpoint_never_runs_a_filter_the_repository_defines`, `crates/interface-review/tests/suite/repository_programs.rs::a_filter_from_a_file_the_repository_settings_include_never_runs`
- **[fix]** `6444b00` · 2026-11-23T18:22:20-06:00 · **Sign builds hardened, timestamped and team-pinned so installed copies keep updating** (`fix/public-signing-and-updates-20261123`)
  - Notes: The next promoted build arrives through Restart to update as usual; in that build Settings > Updates shows updates on, not off.
  - Pins: `crates/interface-updater/src/signature.rs::codesign_requirements_of_real_builds_are_read_with_exists_spelled_out`, `xtask/src/package.rs::an_app_without_the_hardened_runtime_timestamp_and_team_is_refused`, `electron/scripts/sign-mac.test.cjs::electron-osx-sign emits the exact hardened, timestamped and team-pinned arguments`, `electron/scripts/smoke-packaged.test.mjs::a packaged build with a public update channel must come up with updates on`
- **[record]** `473eed8` · 2026-11-21T12:54:24-06:00 · **Record the launch review's third pass: privacy page, ask-first defaults, folder trust** (`docs/launch-review-record-20261121`)
- **[fix]** `a1fb62d` · 2026-11-21T12:53:03-06:00 · **Trust this folder? is asked once; until trusted, Claude runs without the folder's own settings** (`feat/folder-trust-ui-20261121`)
  - Notes: Start a Claude thread in a folder never opened in Claude Code: 'Trust this folder?' appears before the first message is sent
  - Pins: `crates/engine-claude/src/adapter_tests.rs::an_untrusted_folders_own_settings_never_load`, `crates/interface-settings/src/folder_trust.rs::a_folder_cannot_claim_to_be_a_worktree_of_a_repository_it_was_never_added_to`, `crates/interface-desktop-services/src/service/folder_trust_tests.rs::a_repository_cloned_into_an_agents_home_is_not_trusted_through_the_home`, `apps/interface-host/tests/suite/folder_trust.rs::an_untrusted_folders_thread_runs_on_user_settings_until_trusted_then_resumes_with_its_own`, `electron/src/renderer/folder-trust.test.ts::runs the whole question before a new thread's first prompt, answer first`
- **[fix]** `0477fca` · 2026-11-21T12:36:55-06:00 · **Agents ask before a web search or acting on another pane; each has a switch to allow it** (`fix/ask-first-defaults-20261121`)
  - Notes: Settings > Threads shows 'Pane tools without asking', off. An agent's work settings show 'Search the web without asking', off
  - Pins: `crates/interface-desktop-services/src/agent_policy.rs::a_teammates_web_search_asks_unless_its_owner_switched_searching_on`, `crates/interface-desktop-services/src/agent_policy.rs::the_web_search_switch_does_not_loosen_web_fetches_or_anything_else`, `apps/interface-host/src/bootstrap_tests.rs::pane_tools_that_act_on_other_panes_ask_unless_the_owner_switched_them_on`, `crates/interface-settings/src/threads_tests.rs::the_pane_tools_switch_lives_beside_the_store_and_anything_but_on_means_ask`
- **[feature]** `55a1272` · 2026-11-21T12:24:00-06:00 · **Settings gets a Privacy page; a one-time privacy summary; a note beside switching accounts** (`feat/privacy-page-20261121`)
  - Notes: Open Settings: Privacy sits after Updates and lists what leaves this Mac. A small card bottom-right says the same, once
  - Pins: `electron/src/renderer/privacy.test.ts::says what leaves this Mac in the owner's words`, `electron/src/renderer/privacy.test.ts::shows once per profile, until either answer`, `electron/src/renderer/privacy.test.ts::is a corner status card with Open Privacy and Got it, never a dialog`, `electron/src/renderer/privacy.test.ts::goes under a thread's limit line only when it offers another account`
- **[feature]** `5c2f97d` · 2026-11-21T12:03:11-06:00 · **Update channel points at GitHub releases; nothing published yet reads as up to date** (`feat/update-channel-github`)
  - Notes: Settings > Updates > Check for updates still says up to date; nothing is published on the public channel yet
  - Pins: `crates/interface-updater/src/remote.rs::a_feed_with_nothing_published_yet_is_not_a_fault`, `crates/interface-updater/src/service.rs::a_feed_with_nothing_published_yet_reads_up_to_date_even_when_asked`, `crates/interface-updater/src/remote.rs::the_shipped_client_reads_a_missing_manifest_as_nothing_published`
- **[fix]** `4c8bcad` · 2026-11-21T12:00:33-06:00 · **Pull-request watch ignores strangers; dropped text is pasted; app offers to move to Applications** (`fix/launch-review-batch1`)
  - Notes: Drag text from a web page onto a terminal: it shows at the prompt and waits for Return. Nothing else looks different
  - Pins: `crates/interface-review/src/thread_ship_tests.rs::a_watch_hears_only_the_projects_own_reviewers`, `apps/interface-host/src/threads_schedules_tests.rs::a_strangers_review_comment_never_reaches_a_watched_thread`, `electron/src/renderer/terminal-drop.test.ts::drops every other control character, so a drag cannot press keys of its own`, `electron/src/renderer/terminal-drop.test.ts::reaches the program as a paste, never as typed input`, `electron/src/main/applications-folder.test.ts::is made to a packaged app opened from anywhere else on a Mac`
- **[feature]** `8ec6042` · 2026-11-21T11:39:03-06:00 · **Public update channel, switched off: signed releases reach installed copies with no re-download** (`feat/public-update-channel`)
  - Notes: Nothing changes on screen yet: Settings > Updates works as before; the public channel stays off until its address and keys are set
  - Pins: `crates/interface-updater/src/remote.rs::a_newer_signed_release_lands_in_the_local_feed_byte_for_byte`, `crates/interface-updater/src/remote.rs::an_archive_that_is_not_the_signed_bytes_never_reaches_the_installer`, `crates/interface-updater/src/remote.rs::an_older_release_offered_again_is_refused_once_a_newer_one_was_seen`, `crates/interface-updater/src/signed_manifest.rs::a_manifest_signed_by_anyone_else_is_refused`, `crates/interface-updater/src/service.rs::a_published_release_is_downloaded_checked_and_handed_to_the_installer`, `apps/interface-host/src/updater.rs::a_build_with_a_public_channel_ignores_a_feed_from_the_environment`
- **[fix]** `75ca85f` · 2026-11-17T21:28:47-06:00 · **Phone access stays reachable when someone floods the Mac's phone listener or relay** (`fix/phone-connection-limits`)
  - Notes: With phone access on, the iPhone still connects nearby and through the relay; nothing looks different
  - Pins: `crates/interface-remote/src/http/direct_tests.rs::one_address_holding_slow_connections_never_shuts_a_phone_out`, `crates/interface-remote/src/http/direct_tests.rs::a_trickled_request_head_loses_its_connection_at_the_deadline`, `crates/interface-remote/src/http/direct_tests.rs::a_phone_on_a_weak_network_is_still_answered`, `crates/interface-remote/src/http/direct_tests.rs::strangers_holding_relay_sockets_never_keep_the_mac_out`, `crates/interface-remote/src/http/direct_tests.rs::slow_uploads_to_the_relay_never_keep_the_mac_or_a_phone_out`, `crates/interface-remote/src/http/direct_tests.rs::the_mac_comes_back_after_its_relay_restarts`, `crates/interface-remote/src/relay_admission.rs::a_full_address_displaces_only_its_own_oldest_stranger`
- **[fix]** `94fa1c4` · 2026-11-17T10:10:51-06:00 · **Harden approval gates, voice redaction, browser downloads and updater checks** (`fix/security-audit-hardening`)
  - Notes: Voice mode hides passwords, and untrusted repos, downloads and updates run stricter safety checks
  - Pins: `electron/src/renderer/voice-ui.test.ts::omits password input values and data-voice-redact text and values from snapshots`, `crates/engine-claude/src/adapter_tests.rs::auto_mode_escalations_and_init_fallback_ask_the_user`, `crates/interface-updater/src/service.rs::retained_archive_reverify_refuses_tampered_install`
- **[record]** `a0b89a4` · 2026-11-16T11:16:36-06:00 · **Record the 0.3.181 release** (`record-0-3-181`)

### v0.3.181 — 2026-11-16T11:12:41-06:00 (`aab6bfa`)

- **[look]** `71cedd9` · 2026-11-16T11:05:37-06:00 · **Live backgrounds: the sidebar is frosted glass** (`look/frosted-sidebar-20261116`)
  - Notes: Pick a live background in Dev: the sidebar shows soft blurred colour, with no lines or shapes behind the workspace names
  - Pins: `electron/src/renderer/theme-look.test.ts::frosts the sidebar from a still copy of the picture, not by blurring behind it on every frame`, `electron/src/renderer/theme-look.test.ts::leaves the sidebar's frosted copy still between refreshes, then brings it up to the picture`
- **[look]** `10548fe` · 2026-11-16T10:38:19-06:00 · **Threads take the TN Code look: tiles, bubble with a tail, stamps, round composer** (`feat/threads-tn-look`)
  - Notes: Open Threads: rows lead with an initials tile, your prompt is a bubble with a tail under a timestamp, and the composer is a rounded card with a round Send
  - Pins: `electron/src/renderer/thread-transcript.test.ts::stamps the first timed prompt, then one an hour or more after the last stamp`, `electron/src/renderer/thread-inbox.test.ts::shows the first letters of the title's first two words, in capitals`, `electron/src/renderer/components/ThreadComposer.test.ts::sits on the transcript's reading lane as the canvas's card`
- **[record]** `6cb7f7f` · 2026-11-15T22:35:01-06:00 · **Record the pane shortcuts and Rex-style fade landings and their open owner checks** (`todo-cmd-w-checks`)
- **[record]** `a070891` · 2026-11-15T22:34:23-06:00 · **Record the 0.3.179 and 0.3.180 releases and the automatic thread titles** (`docs/release-0180-record-20261115`)
- **[look]** `b462306` · 2026-11-15T22:33:48-06:00 · **Live backgrounds: panes you are not in fade, the way Rex does it** (`pane-fade-inactive-looks`)
  - Notes: Pick a live background (Orbit) with two or more panes: the selected pane is at full strength and the others are faded; click another pane and the fade follows.
  - Pins: `electron/src/renderer/theme-look.test.ts::fades every pane but the selected one, the way Rex does, and never a pane that is waiting`

### v0.3.180 — 2026-11-15T22:29:30-06:00 (`97f0c0e`)

- **[feature]** `6799ba3` · 2026-11-15T22:28:55-06:00 · **⌘↩ opens a terminal pane and ⌘W closes the selected pane instead of the window** (`pane-focus-contrast-shortcuts`)
  - Notes: In Code, press ⌘↩ in a pane: a new shell opens beside it. Press ⌘W: the selected pane closes and the window stays open. ⇧⌘W closes the window.
  - Pins: `electron/src/renderer/terminal-keys.test.ts::opens a pane with ⌘↩ and leaves every other Enter to the program`, `electron/src/renderer/workspace.test.ts::⌘W closes the selected pane, never the window`, `electron/src/main/app-menu.test.ts::⌘W is Close Pane and Close Window moves to ⇧⌘W`
- **[feature]** `ede3a86` · 2026-11-15T22:25:30-06:00 · **Threads are named by Claude Haiku 5.5 from the first message, on by default** (`feat/haiku-thread-titles-20261115`)
  - Notes: Send a new thread's first message: within a second or two its title becomes a short generated one, in the pane head and the list, while the agent still works
  - Pins: `apps/interface-host/src/threads_tests.rs::a_threads_title_is_asked_for_as_its_first_prompt_goes_out_without_waiting_for_its_run`, `crates/interface-settings/src/threads_tests.rs::generated_titles_are_turned_on_once_and_an_owners_later_off_stays_off`, `crates/engine-claude/src/title_tests.rs::a_title_is_named_from_the_first_prompt_alone`
- **[look]** `b205155` · 2026-11-15T22:18:42-06:00 · **Terminal text matches Rex: MonoLisa Bold at 11.5, same rows and columns** (`feat/terminal-rex-type`)
  - Notes: Open a terminal pane: the text is MonoLisa Bold, smaller and tighter like a Rex pane, and Settings shows Terminal text size 11.5 px
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::uses the owner's terminal face, MonoLisa Bold, instead of the app chrome face`, `electron/src/renderer/terminal-cell.test.ts::matches the owner's Rex pane: MonoLisa 11.5px is 15 device pixels wide on Retina`, `crates/interface-desktop-services/src/service_tests.rs::workspace_presentation_v6_keeps_half_sizes_and_older_surfaces_read_whole_ones`
- **[look]** `a948804` · 2026-11-15T22:03:07-06:00 · **Pane headers show only the name, status dot and icons; Find, Files, folder and status words are gone** (`look/pane-head-dot-only-20261115`)
  - Notes: Every terminal pane's top bar shows only its name, the small status dot and the icons on the right; no Find, Files, folder name, Idle or Working · time
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::draws no folder, no state in words, no Find and no Files: a tab's dot is its state`

### v0.3.179 — 2026-11-15T21:43:06-06:00 (`386483a`)

- **[record]** `751ae8f` · 2026-11-15T21:37:12-06:00 · **Record the UI redesign landings from the design canvas** (`docs/ui-redesign-record-20261115`)
- **[look]** `911a77b` · 2026-11-15T21:36:30-06:00 · **Settings fills the window: one row style, plain words, Live backgrounds first** (`ui/settings-page-20261115`)
  - Notes: Open Settings: it takes the whole window with its own menu; Appearance starts with Live backgrounds and a Motion menu; Updates shows What's new in plain words
  - Pins: `electron/src/renderer/components/SettingsPage.test.ts::ends the menu with the version and notes the pages that want you`, `electron/src/renderer/components/SettingsPage.test.ts::sets how live backgrounds move with one Motion menu`, `electron/src/renderer/components/SettingsPage.test.ts::opens each provider's accounts from its own Accounts menu`, `electron/src/renderer/components/SettingsPage.test.ts::leads the picker with the theme files that move, under Live backgrounds`
- **[look]** `c2d8594` · 2026-11-15T21:26:46-06:00 · **Pane headers say name, folder and status in words; Find and Files move into the header** (`ui/pane-header-20261115`)
  - Notes: A terminal pane's head reads like 'Claude Code · home · Working · 2 min', with Find and Files beside the ⋯ menu; nothing floats over the terminal text
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::names the pane, its folder and its state in words, then Find and Files before the icons`, `electron/src/renderer/components/TerminalPrimitives.test.ts::draws nothing over the text: search, copy and paste wait for the pane head's request`
- **[look]** `90fc6cd` · 2026-11-15T21:22:07-06:00 · **Sidebar footer is one quiet row, and errors are small notices in the corner** (`ui/shell-footer-notices-20261115`)
  - Notes: The sidebar's foot reads Settings with a small amber 'providers need setup'; an error shows as a small card bottom-right with Try again and Dismiss
  - Pins: `electron/src/renderer/components/Sidebar.test.ts::puts Settings and the setup notice in one quiet footer row, without the version`, `electron/src/renderer/corner-notices.test.ts::names the provider by its display name in one sentence`
- **[look]** `dc43b9a` · 2026-11-15T21:17:38-06:00 · **Threads: a short title over one plain line, and a kept draft inside the composer** (`ui/thread-view-20261115`)
  - Notes: Open a thread: its title is one short line, then project · branch · where edits land. A message that was not sent shows inside the composer
  - Pins: `electron/src/renderer/thread-title.test.ts::takes the prompt's first line, collapses its white space and cuts it on a word near 60 characters`, `electron/src/renderer/components/PromptRecoveryPanel.test.ts::shows only a message with something to recover, never one delivered and answered`
- **[record]** `7d44917` · 2026-11-15T20:26:19-06:00 · **Record the Live backgrounds landing and the release still waiting** (`docs/review-record-live-20261115`)
- **[look]** `1115be0` · 2026-11-15T20:25:43-06:00 · **Settings: themes with a moving backdrop lead the theme picker as Live backgrounds** (`look/live-backgrounds-group-20261115`)
  - Notes: Settings › Appearance: the theme picker starts with Live backgrounds (Aurora, Glyph, Neon Grid, Orbit, Paper Tide, TN Code); click one to turn it on
  - Pins: `electron/src/renderer/components/SettingsPage.test.ts::leads the picker with the theme files that move, under Live backgrounds`
- **[record]** `045b9af` · 2026-11-15T18:25:10-06:00 · **Record the review of the 14-15 November features** (`docs/review-record-20261115`)
- **[fix]** `4033fe4` · 2026-11-15T18:23:25-06:00 · **A prompt fired at launch keeps the thread's history; visual replies cannot reach the network** (`fix/sched-visuals-review-20261115`)
  - Notes: Quit with a schedule due, reopen: the thread shows its earlier conversation and the new turn. A link clicked in a visual no longer blanks the card
  - Pins: `electron/src/renderer/history-late-read.test.ts::still reads its history once and shows it in order, the live turn once`, `electron/src/main/visual-network-fence.test.ts::turns off WebRTC's own UDP and sends every request, loopback too, to a proxy that leads nowhere`, `electron/src/main/visual-network-fence.test.ts::fences the window's own session before its first document loads`, `crates/interface-settings/src/thread_schedules_tests.rs::a_firing_waiting_for_its_thread_comes_first_in_the_view`
- **[fix]** `47c6e43` · 2026-11-15T18:18:01-06:00 · **Threads search finds old messages in long threads and runs 3x faster; a crashed turn reads stopped** (`fix/journal-review-20261115`)
  - Notes: Search Threads for a word from early in a long older thread: it is found, with the word shown. After a crash a thread no longer shows Working forever
  - Pins: `apps/interface-host/src/threads_tests.rs::an_eight_hundred_item_history_is_imported_whole`, `apps/interface-host/src/threads/journal_tests.rs::a_journal_imported_short_is_rebuilt_once_from_the_whole_history`, `apps/interface-host/src/threads/journal_tests.rs::a_failed_rebuild_keeps_the_old_journal`, `crates/interface-settings/src/thread_journal/tests.rs::search_skips_unparsed_lines_without_changing_what_it_finds`, `apps/interface-host/src/bootstrap_tests.rs::a_session_left_working_by_a_host_crash_opens_idle_without_rewriting_the_cache`, `electron/src/renderer/thread-inbox.test.ts::starts an In messages excerpt just before the match, so the matching text shows on line two`
- **[internal]** `ec11a15` · 2026-11-15T18:13:16-06:00 · **The Browser test clicks a new tab only once it takes pointer input** (`fix/browser-lane-input-20261115`)
  - Notes: The Browser lane clicks a page only once it takes pointer input
- **[fix]** `77303e7` · 2026-11-15T17:39:41-06:00 · **Composer: a secret card goes when its agent is gone; stash never doubles or drops typed words** (`fix/composer-review-20261115`)
  - Notes: Ask an agent for a secret, then press Stop: the card disappears and the thread can ask again. ⌘S while typing leaves each word in one place only
  - Pins: `apps/interface-host/src/threads_team_tests.rs::a_pending_secret_is_withdrawn_when_its_connection_closes`, `apps/interface-host/src/threads_team_tests.rs::a_pending_secret_is_withdrawn_when_its_turn_ends`, `apps/interface-host/src/threads_team_tests.rs::a_late_end_of_the_previous_turn_never_withdraws_the_next_turns_secret`, `electron/src/renderer/use-draft-stash.test.ts::keeps only the words typed while Rust stored the draft, so nothing is in two places`, `electron/src/renderer/use-draft-stash.test.ts::keeps the words typed while a restore was on its way, after the restored draft`
- **[fix]** `aa9a026` · 2026-11-15T17:30:02-06:00 · **After a provider switch, the Changes count and Whole thread diff cover the whole thread** (`fix/switch-totals-20261115`)
  - Notes: Switch a thread Claude → Grok → Claude: the pane head's Changes +N matches the Changes panel's Whole thread, which lists every file
  - Pins: `crates/interface-settings/src/threads/chain_tests.rs::a_switched_threads_view_carries_every_segments_line_totals`, `apps/interface-host/src/threads_switch_tests.rs::an_acp_launch_handoff_starts_on_a_checkpoint_taken_before_the_agent`, `apps/interface-host/src/threads_switch_tests.rs::a_thread_switched_back_is_diffed_from_its_earliest_checkpoint`, `electron/src/renderer/thread-inbox.test.ts::totals a switched thread's every segment in the head and the row, and an unswitched one as before`
- **[fix]** `31a49c4` · 2026-11-15T17:22:03-06:00 · **Looks: a theme can no longer move behind another app; the play mark shows only on themes that move** (`fix/looks-review-20261115`)
  - Notes: Settings › Appearance › Yours: a theme that animates only its layers gets the play mark, a still scene does not; no backdrop moves while Interface is behind
  - Pins: `electron/src/renderer/theme-look.test.ts::is held all the same while Interface is behind another app`, `electron/src/renderer/theme-look.test.ts::rests when its only motion is a shader that cannot draw`, `crates/interface-settings/src/custom_themes.rs::marks_a_theme_moving_when_its_backdrop_layers_animate`, `crates/interface-settings/src/custom_themes.rs::marks_a_scene_moving_only_when_something_in_it_moves`
- **[fix]** `6dec0da` · 2026-11-15T17:16:12-06:00 · **A thread shows the permission mode it really runs in, after a reload and for helper threads** (`fix/thread-mode-20261115`)
  - Notes: After a reload a thread's mode chip shows what the agent really runs in (helpers show Accept edits), and the first send no longer switches its mode
  - Pins: `apps/interface-host/tests/suite/session_management.rs::listings_and_the_cached_registry_carry_the_mode_the_host_keeps`, `electron/src/renderer/session-presentation.test.ts::shows the host's mode after a reload even when another mode is saved for the model`, `electron/src/renderer/session-presentation.test.ts::never applies a saved mode before the first send of a session the host has a mode for`, `electron/src/renderer/session-presentation.test.ts::lets the owner pick Manual for a session the host runs in another mode`
- **[fix]** `f50f7d9` · 2026-11-15T17:10:41-06:00 · **Browser: a page opens tabs or windows only after a click; new-tab downloads say where files went** (`fix/browser-review-20261115`)
  - Notes: A page can no longer open tabs or windows by itself; a popup's title starts with its site. A Download link that opens a tab says Saved to Downloads
  - Pins: `electron/src/main/browser-pane.test.ts::refuses a window.open with no press, and opens one per press`, `electron/src/main/browser-pane.test.ts::keeps at most two popup windows open per tab`, `electron/src/main/browser-pane.test.ts::titles a popup window with its site first, whatever the page calls itself`
- **[fix]** `40267c5` · 2026-11-15T17:06:21-06:00 · **A Claude thread at its usage limit offers Switch to on the limit line** (`fix/switch-limit-line-20261115`)
  - Notes: When a Claude thread hits its usage limit, the line above the composer offers Switch to another provider beside Resume at reset
  - Pins: `electron/src/renderer/thread-resume.test.ts::offers Switch to on a Claude limit that its turn's result already left out of the event log`
- **[fix]** `e84d3c1` · 2026-11-15T17:01:17-06:00 · **Accounts: a removed account's threads stay listed and deletable; Sign in uses the thread's account** (`fix/accounts-review-20261115`)
  - Notes: Remove an added account: its threads stay in Threads marked account removed. A signed-out thread's Sign in opens that thread's own account
  - Pins: `apps/interface-host/src/provider_accounts_removed_tests.rs::a_removed_accounts_sessions_stay_listed_after_the_next_listing`, `apps/interface-host/src/provider_accounts_removed_tests.rs::a_removed_accounts_sessions_stay_listed_and_bound_after_a_restart`, `apps/interface-host/src/provider_accounts_removed_tests.rs::a_removed_accounts_session_can_be_deleted_and_does_not_come_back`, `electron/src/renderer/thread-sign-in-account.test.ts::signs in the account the thread runs on, not the provider's default account`
- **[fix]** `b2f1a1f` · 2026-11-15T16:54:45-06:00 · **Team tools: starts sent together keep the 4-helper limit; finish notices arrive one at a time** (`fix/team-tools-review-20261115`)
  - Notes: Six helpers started at once stop at four running; helpers that finish together each show their own finished message in the caller
  - Pins: `apps/interface-host/src/threads_team_tests.rs::starts_at_once_never_pass_the_running_limit_together`, `apps/interface-host/src/threads_team_tests.rs::notices_of_threads_that_finish_together_go_one_turn_at_a_time`, `apps/interface-host/src/threads_team_tests.rs::a_team_call_before_its_bind_waits_when_the_project_path_is_a_link`
- **[fix]** `4d195a5` · 2026-11-15T16:50:20-06:00 · **Outside agents: a retried start no longer makes a second thread; long calls and project names work** (`fix/outside-agents-review-20261115`)
  - Notes: No change on screen: an outside agent (iface mcp) retrying a start gets the same thread and can list threads by a project's name
  - Pins: `apps/interface-host/src/threads_outside_tests.rs::an_outside_start_retried_from_a_new_session_answers_the_first_thread`, `apps/interface-host/src/threads_outside_tests.rs::an_outside_list_filters_by_a_project_name_as_well_as_its_root`, `apps/iface/src/mcp_tests.rs::a_call_longer_than_interface_reads_is_refused_unsent_and_the_session_goes_on`, `apps/iface/src/mcp_tests.rs::a_connection_interface_closes_unanswered_is_not_called_interface_not_running`
- **[look]** `572f062` · 2026-11-15T14:45:38-06:00 · **Theme files that can move carry a play mark in Settings** (`feat/moving-theme-mark-20261115`)
  - Notes: Settings › Appearance › Yours: Aurora, Paper Tide, Neon Grid, TN Code, Orbit and Glyph show a small play mark
  - Pins: `electron/src/renderer/components/SettingsPage.test.ts::marks the theme files whose backdrop can move`, `crates/interface-settings/src/custom_themes.rs::says_whether_a_theme_paints_a_backdrop_and_whether_it_can_move`

### v0.3.178 — 2026-11-15T14:34:05-06:00 (`78ca45a`)

- **[fix]** `685f81c` · 2026-11-15T14:33:14-06:00 · **Terminal text is the terminal font again, not tiny sans-serif** (`fix/terminal-font-20261115`)
  - Notes: Terminal panes show normal-size monospace text again, in Dev and after updating the regular app
  - Pins: `electron/src/renderer/canvas-font.test.ts::is drawn in the font xterm asked for, not the canvas's 10px default`, `electron/src/renderer/canvas-font.test.ts::is in place before any terminal is made`
- **[record]** `f070f57` · 2026-11-15T13:57:13-06:00 · **Record the TN Code themes landing** (`feat/tn-code-themes-20261115`)
- **[feature]** `b6274e1` · 2026-11-15T13:56:42-06:00 · **TN Code's themes come to Interface, with shader backdrops for Orbit and Glyph** (`feat/tn-code-themes-20261115`)
  - Notes: After copying the themes: TN Code, Orbit and Glyph show their moving patterns behind the panes in Dev
  - Pins: `electron/src/renderer/theme-look.test.ts::draw Orbit's horizon behind TN Code and Orbit, and Glyph's dots behind Glyph`, `electron/src/renderer/theme-look.test.ts::is drawn when its own rate says a frame is due, under the backdrop's stepper`
- **[fix]** `95820c9` · 2026-11-15T13:52:33-06:00 · **Browser pages rest under flipped cards, and dropped downloads say they failed** (`fix/browser-followups`)
  - Notes: Flip panes to cards with a video page open: the page stops running underneath; a download that loses its connection says it could not be downloaded
  - Pins: `electron/src/main/browser-pane.test.ts::fails a download whose connection drops, frees its name and cancels it`
- **[record]** `a1febe5` · 2026-11-15T12:57:54-06:00 · **Record the looks battery landing** (`perf/looks-battery-20261115`)
- **[speed]** `08e5b62` · 2026-11-15T12:57:20-06:00 · **Moving backdrops cost about half as much, and rest on battery when idle** (`perf/looks-battery-20261115`)
  - Notes: Pick Aurora in Dev: it still moves, and Settings › Appearance shows Still on battery under Moving backdrops
  - Pins: `electron/src/renderer/theme-look.test.ts::is drawn half as often on the battery, and rests there after a minute without input`, `electron/src/renderer/theme-look.test.ts::writes a blur into the page only when the theme file asks for one`
- **[feature]** `03af8a8` · 2026-11-15T10:55:40-06:00 · **The in-app Browser works like a Chrome tab** (`feature/browser-chrome-like`)
  - Notes: Right-click menus, new tabs from links, sign-in popups, downloads, ⌘F, zoom, camera/mic prompts, fullscreen video and PDFs work in a Browser tab
  - Pins: `electron/src/main/browser-pane.test.ts::asks for the camera, microphone, notifications and the clipboard, allows fullscreen and refuses the rest`, `electron/src/main/browser-pane.test.ts::opens new tabs and windows the way Chrome does`, `electron/src/main/browser-pane.test.ts::makes Block the sheet's default, so only a deliberate Allow grants`, `electron/src/main/browser-pane.test.ts::pins a popup window exactly as a page, with no preload and no Node`
- **[record]** `803b338` · 2026-11-15T10:33:40-06:00 · **Record the looks landing** (`feat/theme-looks-20261115`)
- **[feature]** `b3a0466` · 2026-11-15T10:33:05-06:00 · **Looks: theme files can paint a moving backdrop behind glass panes and threads** (`feat/theme-looks-20261115`)
  - Notes: Pick Aurora under Yours in Settings › Appearance: one moving picture runs behind the sidebar, panes and threads
  - Pins: `electron/src/renderer/theme-look.test.ts::turns panes, threads and the sidebar to glass only while a backdrop is painted`, `electron/src/renderer/theme-look.test.ts::runs only while Interface is in front, motion is allowed and Settings leaves it on`, `electron/src/renderer/theme-look.test.ts::keeps what draws and drops scripts, foreign content, links and handlers`
- **[look]** `0fd4320` · 2026-11-15T10:12:28-06:00 · **Browser opens as a tab in the pane you open it from** (`look/browser-tab-in-pane`)
  - Notes: Choosing Browser from a pane's + adds a Browser tab to that pane with the address bar ready; no pane splits or moves
  - Pins: `electron/src/renderer/terminal-workspace.test.ts::opens from a pane as a tab in that pane, shown and focused, with no pane added or moved`
- **[record]** `13577d3` · 2026-11-15T09:03:16-06:00 · **Record the 0.3.177 release of the T3-style features** (`record/t3-release-20261115`)

### v0.3.177 — 2026-11-15T09:00:21-06:00 (`5404c46`)

- **[fix]** `0e2711d` · 2026-11-15T04:26:38-06:00 · **A new thread's first team tool call waits for the thread instead of saying it has no project** (`fix/team-tools-first-prompt-20261115`)
  - Notes: An agent asked in a new thread's first prompt to start or read threads does it, instead of saying the thread has no project folder
  - Pins: `apps/interface-host/src/threads_team_tests.rs::a_team_call_before_its_threads_bind_waits_for_the_bind`
- **[record]** `d8bf4e1` · 2026-11-15T04:19:18-06:00 · **Record the twelve T3-style features as landed, with follow-ups and owner decisions** (`record/t3-systems-20261115`)
- **[feature]** `5d45ca8` · 2026-11-15T04:17:33-06:00 · **Quick wins: Write with AI for commit and PR text, a private secret card, and side questions** (`feat/quick-wins-20261115`)
  - Notes: Write with AI fills commit and PR text; an agent's secret request takes a value into a private file only; /btw asks a side question shown on the right
  - Pins: `apps/interface-host/src/threads_ship_tests.rs::write_with_ai_reads_only_what_ships_and_changes_nothing`, `apps/interface-host/src/threads_team_tests.rs::a_given_secret_exists_only_in_its_owner_only_file`, `apps/interface-host/src/threads_team_tests.rs::a_side_question_forks_the_thread_hidden_and_read_only_and_goes_when_closed`
- **[feature]** `62b88b6` · 2026-11-15T03:48:33-06:00 · **Resume at reset: a thread that hits its usage limit carries on by itself after the reset** (`feat/resume-at-reset-20261115`)
  - Notes: When a thread hits its usage limit, the line offers Resume at reset and Snooze until reset; Resume carries the thread on by itself a minute after the reset
  - Pins: `apps/interface-host/src/threads_schedules_tests.rs::resume_at_reset_carries_the_thread_on_a_minute_after_the_reset_once`, `electron/src/renderer/thread-resume.test.ts::draws Resume at reset and Snooze until reset beside Switch to, and alone when there is nowhere to go`
- **[fix]** `e3ab161` · 2026-11-15T03:44:24-06:00 · **A thread whose turn finishes quickly no longer shows as stopped instead of done** (`fix/finished-run-not-stopped-20261115`)
  - Notes: A turn that finishes in a moment reads done and shows as unread; team waits, notices and scheduled prompts no longer see a finished run as stopped
  - Pins: `apps/interface-host/src/threads_tests.rs::a_finished_run_whose_end_is_still_queued_is_not_filed_as_stopped_by_a_reconcile`
- **[feature]** `56a16ba` · 2026-11-15T03:31:07-06:00 · **Outside agents: Claude Code or Codex in a terminal can read and start threads (iface mcp)** (`feat/outside-agents-20261115`)
  - Notes: Settings, Threads, Outside agents lets a Claude Code or Codex in any terminal read your threads and, if allowed, start ones that ask before acting
  - Pins: `apps/interface-host/tests/suite/outside_agents.rs::iface_mcp_lists_starts_waits_for_and_reads_a_thread_on_a_real_host`, `apps/interface-host/src/threads_outside_tests.rs::an_outside_start_runs_in_a_known_project_at_the_asking_rung_without_team_tools`
- **[feature]** `efd7234` · 2026-11-15T03:06:15-06:00 · **Quote a reply, send terminal text or another thread into a draft, and stash drafts** (`feat/composer-references-20261115`)
  - Notes: Select reply text and click Quote; Cmd-S stashes a draft and Stashed (n) brings it back; a Thread chip sends that thread's title and last exchange
  - Pins: `apps/interface-host/src/threads_references_tests.rs::a_thread_reference_becomes_the_block_rust_writes_from_the_journal`, `electron/src/renderer/composer-quote.test.ts::goes after the typed draft, one blank line apart, with the caret after it`
- **[feature]** `221227b` · 2026-11-15T02:40:06-06:00 · **Scheduled prompts in a thread, and Watch on a pull request wakes the agent** (`feat/thread-schedules-20261115`)
  - Notes: Schedule a prompt in a thread's menu sends it later as a folded Scheduled prompt the agent answers; a Next line shows above the composer
  - Pins: `apps/interface-host/src/threads_schedules_tests.rs::a_due_schedule_fires_once_into_an_idle_thread_as_a_host_prompt`, `electron/src/renderer/thread-schedules.test.ts::refuses an interval under five minutes, a past time and an empty prompt`
- **[feature]** `fde043e` · 2026-11-15T02:31:57-06:00 · **Visual replies: an agent can show a chart or mockup as a rendered card above its reply** (`feat/visual-replies-20261115`)
  - Notes: Ask an agent in a thread for a chart: a card shows the drawn page above its reply, with Source and Open larger; it is still there after reopening
  - Pins: `apps/interface-host/src/threads_team_tests.rs::show_visual_keeps_journals_and_shows_the_page_once_and_refuses_with_codes`, `electron/src/main/visual-frames.test.ts::lets only an empty subframe load a visual, and nothing move one afterwards`, `electron/src/shared/visual-frame.test.ts::gives the page no network, storage or forms: only inline code and data: media`
- **[feature]** `105b4db` · 2026-11-15T02:08:53-06:00 · **Several accounts per provider: add accounts in Settings and pick one for a new thread** (`feat/provider-accounts-20261115`)
  - Notes: Settings, Models and providers lists each provider's accounts; a new thread can run on another Claude, Codex or Grok account, and its head names it
  - Pins: `apps/interface-host/src/provider_accounts_tests.rs::a_session_launched_on_an_account_is_bound_and_resumes_and_reads_history_there`, `electron/src/renderer/composer-thread.test.ts::puts an account control beside the model only when the provider has several accounts`
- **[feature]** `9663273` · 2026-11-15T01:43:33-06:00 · **Switch to <provider> carries a thread on another provider and back, in the same thread** (`feat/switch-provider-20261115`)
  - Notes: Switch to Grok in a thread's menu keeps the same row and conversation; Grok gets a handoff, and switching back tells Claude only what it missed
  - Pins: `apps/interface-host/src/threads_switch_tests.rs::switching_to_a_new_provider_hands_the_same_thread_over_from_its_journal`, `electron/src/renderer/thread-switch.test.ts::moves the pane, draft and attachments to the answered head, then shows it`
- **[record]** `055cc8a` · 2026-11-15T01:35:21-06:00 · **To-do list tidied: finished items move to the done file; code review fixes recorded** (`record/review-fixes-todo`)
  - Notes: The to-do list keeps open work; finished items move to the done file
- **[internal]** `5975572` · 2026-11-15T01:32:54-06:00 · **Electron security update (43.8.10) and dev-tool advisory fixes** (`internal/dependency-advisories`)
  - Notes: The app runs on Electron 43.8.10, and the dev tools carry their security fixes
- **[internal]** `630d1ae` · 2026-11-15T01:26:30-06:00 · **Collaboration contract code split into smaller files** (`internal/collaboration-state-split`)
  - Notes: The collaboration contract file is split into five modules along its sections
- **[fix]** `9e3f336` · 2026-11-15T01:20:06-06:00 · **A finished Codex turn leaves its thread finished and unread, not stuck in an open run** (`fix/codex-idle-thinking-no-run-20261115`)
  - Notes: After a Codex reply finishes, its thread reads finished and unread right away instead of still running until the session closes
  - Pins: `apps/interface-host/src/threads_tests.rs::thinking_that_stops_opens_no_run_but_still_counts_inside_one`, `apps/interface-host/tests/suite/provider_replay.rs::codex_simple_replays_through_the_runtime_and_threads`
- **[fix]** `31dbf43` · 2026-11-15T01:13:13-06:00 · **Stopping a Claude turn ends it as interrupted, not failed, and opens no extra thread run** (`fix/claude-stop-not-failure-20261115`)
  - Notes: Stop a Claude reply mid-answer: the thread shows the turn as stopped, the session goes idle, and no error appears
  - Pins: `apps/interface-host/tests/suite/provider_replay.rs::claude_interrupt_replays_through_the_runtime_and_threads`
- **[internal]** `d35d510` · 2026-11-15T01:09:17-06:00 · **Away's worker runner sheds leftover pane-delegation code** (`internal/delegation-leftovers`)
  - Notes: The Away worker runner drops the pane-delegation parts nothing used
- **[fix]** `8056d21` · 2026-11-15T00:59:27-06:00 · **Old finished team workers stay out of Threads and the Chat list** (`fix/threads-hide-old-workers`)
  - Notes: After many agent teams, old finished workers no longer reappear as new threads at the top of Threads or as rows in the Chat list
  - Pins: `apps/interface-host/src/threads_tests.rs::an_old_finished_worker_past_the_snapshot_bound_never_becomes_a_thread`, `apps/interface-host/src/orchestration_transport.rs::owner_and_client_name_every_worker_past_the_snapshot_bound`, `crates/interface-orchestration/tests/orchestration.rs::worker_session_keys_name_every_worker_the_control_snapshot_drops`, `electron/src/renderer/use-threads.test.ts::keeps the hidden sessions' identity while a newer view names the same ones`
- **[internal]** `56e0755` · 2026-11-15T00:47:41-06:00 · **The gate catches update-log format changes, stale JS installs and hung Rust tests** (`internal/gate-receipt-and-timeout`)
  - Notes: A change to the update log's format now runs the Electron parser test too; The Electron gate receipt now follows the installed node_modules, not only the lockfile; A hung Rust test executable now fails the gate after ten minutes instead of holding it forever
- **[feature]** `2149d26` · 2026-11-15T00:36:49-06:00 · **Agents in a thread can start, wait for, read and message other threads** (`feat/team-tools-20261114`)
  - Notes: Ask an agent to start a helper thread: it shows in the inbox as Started by that thread, and the first agent gets its answer back
  - Pins: `apps/interface-host/src/threads_team_tests.rs::start_thread_files_a_child_thread_sends_its_task_and_never_widens_its_mode`, `crates/interface-settings/src/thread_team_tests.rs::a_started_thread_never_runs_wider_than_its_caller_for_every_engine_pair`
- **[fix]** `2e40c19` · 2026-11-15T00:27:34-06:00 · **Phone access stays reachable when another device ties up the Mac's phone listener** (`fix/phone-listener-limits`)
  - Notes: With phone access on, the iPhone still connects nearby and through the relay; nothing looks different
  - Pins: `crates/interface-remote/src/http/direct_tests.rs::uploads_that_never_send_their_body_hold_no_request_slot`, `crates/interface-remote/src/http/direct_tests.rs::the_listener_caps_connections_and_closes_silent_ones`, `crates/interface-remote/src/http/direct_tests.rs::the_listener_drops_a_peer_outside_the_nearby_networks_before_reading`, `crates/interface-remote/src/http/direct_tests.rs::every_nearby_response_closes_its_connection`
- **[speed]** `c54f34e` · 2026-11-15T00:13:59-06:00 · **Sends and approval answers no longer wait behind an agent's disk writes** (`speed/agent-guard-off-async`)
  - Notes: Sending a message or answering an approval in any chat no longer freezes briefly while an agent is saving its task progress
  - Pins: `apps/interface-host/src/agents_tests.rs::a_send_waits_for_the_agent_store_off_the_async_thread`
- **[speed]** `6bbcf71` · 2026-11-15T00:08:04-06:00 · **The second app copy stops re-checking Muse and Antigravity plugins at every start** (`speed/launch-plugin-marker`)
  - Notes: Whichever of Interface or Interface Dev did not install the checkpoint plugins no longer runs three provider commands before its window connects
  - Pins: `apps/interface-host/src/native_checkpoint_setup.rs::plugins_registered_by_the_other_channel_are_checked_once`
- **[speed]** `70d2985` · 2026-11-15T00:04:25-06:00 · **Saved reviews of images and PDFs keep each snapshot once, in its own file** (`speed/output-reviews-snapshots`)
  - Notes: Typing a review note saves at once even with large images saved, and 32 image or PDF reviews fit instead of about two
  - Pins: `crates/interface-settings/src/output_reviews.rs::a_note_save_rewrites_only_the_small_metadata_file`, `crates/interface-settings/src/output_reviews.rs::a_version_1_store_is_carried_over_once_and_left_in_place`
- **[internal]** `353c788` · 2026-11-15T00:02:38-06:00 · **Recorded Claude and Codex sessions replay in tests; cargo xtask prove checks a fix** (`feat/provider-replay-20261114`)
  - Notes: Provider replay: a test-only recorder and replayer for provider CLI sessions; Provider replay: recorded Claude and Codex sessions now replay through the real runtime and threads; cargo xtask prove: a pinned test must fail on the base and pass here; replay documented; Provider replay: satisfy the gate's lint on one test; Mark provider replay built and awaiting landing; Replay tests build against the thread host that has a journal
- **[fix]** `a3b3775` · 2026-11-14T23:59:06-06:00 · **New chats keep working after 256 recovered sessions** (`fix/prompt-recovery-eviction`)
  - Notes: Starting a new chat or thread never fails with 'Prompt recovery is full' unless every saved prompt is still unresolved
  - Pins: `crates/interface-settings/src/prompt_recovery.rs::a_new_session_past_the_cap_replaces_the_oldest_settled_receipt`, `crates/interface-settings/src/prompt_recovery.rs::unknown_outcomes_and_launch_guards_are_never_evicted`, `crates/interface-settings/src/agent_starts.rs::a_new_request_past_the_limit_replaces_the_oldest_settled_intent`
- **[fix]** `6f27b3c` · 2026-11-14T23:56:25-06:00 · **iface follows provider updates, reports uncertain sends honestly, and spares reused process numbers** (`fix/iface-provider-path-retry-kill`)
  - Notes: An iface left open across a Claude or Codex update keeps working; a send cut off mid-way exits 4 and says check first; iface kill spares a reused pid
  - Pins: `apps/iface/src/app_tests.rs::provider_resolution_keeps_the_link_not_the_version_it_points_at`, `apps/iface/src/agent_client.rs::a_send_whose_reply_is_lost_after_the_write_is_outcome_unknown_not_retryable`, `apps/iface/src/background.rs::a_reused_pid_is_listed_stale_and_never_signalled`
- **[speed]** `f9eb97e` · 2026-11-14T23:50:59-06:00 · **Long Claude chats open without re-reading the whole transcript** (`speed/claude-long-chat-index`)
  - Notes: Reopening a long Claude chat, or loading older messages in it, is quick instead of pausing while the whole transcript is read again
  - Pins: `crates/engine-claude/src/store/history_cache.rs::tail_index_reads_match_fresh_reads_over_the_cache_cap`
- **[fix]** `1e40c85` · 2026-11-14T23:43:02-06:00 · **Copy Diagnostics hides access_token, refresh_token and similar values** (`fix/diagnostics-token-filter`)
  - Notes: A copied diagnostics report shows refresh_token=[redacted] instead of the value
  - Pins: `crates/interface-review/src/diagnostics.rs::copy_diagnostics_names_the_build_and_providers_and_never_carries_tokens`
- **[internal]** `260709a` · 2026-11-14T23:39:48-06:00 · **App's state blocks move into eight hooks; App.tsx is about 850 lines shorter** (`internal/app-hooks`)
  - Notes: The Threads bridges and second-opinion block live in their own hook; The thread output preview's state lives in its own hook; The file picker's store lives in its own hook; The per-turn Undo's capture and restore live in their own hook; The agent store's reads, mutations and poll live in their own hook; The subagent detail column's state lives in its own hook; The return recap lives in its own hook; Project Away's panel state lives in its own hook
- **[internal]** `fac2f79` · 2026-11-14T23:33:23-06:00 · **The iface TUI controller and settings commands move out of tui.rs** (`internal/tui-split`)
  - Notes: Move TuiController's methods and the launch-settings commands out of tui.rs
- **[fix]** `29aef5c` · 2026-11-14T23:28:12-06:00 · **Agents ask before commands hidden behind a zsh short if/while or runners like xcrun and uv run** (`fix/agent-policy-runner-gaps`)
  - Notes: An agent running 'if [[ -n x ]] git push' or 'uv run git push' now asks you first instead of running unasked
  - Pins: `crates/interface-desktop-services/src/agent_policy.rs::policy_table`
- **[feature]** `5e7162c` · 2026-11-14T23:21:32-06:00 · **Threads keep a durable record of every conversation, and the Threads search finds message text** (`feat/thread-journal-20261114`)
  - Notes: Type three or more letters from an old message into the Threads search: the thread appears under In messages with the matching text
  - Pins: `apps/interface-host/src/threads_tests.rs::the_journal_worker_drains_and_its_reads_find_the_answer`, `electron/src/renderer/thread-inbox.test.ts::lists threads whose messages match under In messages, with the excerpt in the project's place`
- **[fix]** `114b45d` · 2026-11-14T23:19:05-06:00 · **Agent Canvas Stop buttons work, and an agent's first chat no longer crashes the window** (`fix/canvas-stop-roster-contract-parity`)
  - Notes: Stop worker, Stop session and Stop all workers on the Agent Canvas now stop the agent; an agent's first message no longer brings up the crash screen
  - Pins: `crates/interface-contracts/src/ipc.rs::orchestration_agent_requests_accept_both_agent_id_spellings`, `apps/interface-host/src/orchestration_tests.rs::stop_agent_accepts_the_camel_case_agent_id_the_app_sends`, `electron/src/renderer/agent-roster.test.ts::reads an agent whose first thread has no past list, as Rust sends it`, `electron/scripts/contract-parity.test.mjs::every same-named Rust struct and TypeScript interface agree on wire fields`
- **[fix]** `6b9fe3d` · 2026-11-14T23:12:27-06:00 · **Quitting cleans up the temporary worktrees of chats still open** (`fix/quit-releases-chat-worktrees`)
  - Notes: Quit with a chat open in a clean throwaway worktree: the worktree is gone after quit and the chat points back at its project folder
  - Pins: `apps/interface-host/src/dispatcher_tests.rs::quit_after_the_desktop_receiver_is_gone_still_schedules_worktree_release`, `apps/interface-host/src/dispatcher_tests.rs::quit_after_an_unforwarded_provider_event_still_projects_close_and_release`, `crates/interface-runtime/tests/runtime.rs::a_registry_change_after_shutdown_reaches_the_cache_when_flushed`
- **[speed]** `8cbc908` · 2026-11-14T23:06:02-06:00 · **The update history loads only when you open Settings → Updates** (`speed/lazy-update-log`)
  - Notes: The app's main script is 20% smaller (2.65 MB to 2.12 MB); the update log appears when you open Settings → Updates
  - Pins: `electron/src/renderer/update-log.test.ts::loads the log and its panel only when Settings -> Updates opens`
- **[feature]** `9165b13` · 2026-11-14T22:57:41-06:00 · **Browser pane: point at an element or mark a region and add it to a thread's draft** (`feat/browser-point-and-fix-20261114`)
  - Notes: In a Browser tab press Pick and click an element: a card shows its picture, and Add to draft puts a note and the picture in the chosen thread's draft
  - Pins: `electron/src/renderer/browser-point.test.ts::appends the note and attaches the picture, never sending`
- **[fix]** `abdff00` · 2026-11-14T22:42:00-06:00 · **Forking a chat no longer says it is safe to retry when the copy may exist** (`fix/fork-outcome-unknown`)
  - Notes: If a fork hits a local error after the copy is made, Interface says the outcome is unknown and does not offer a blind retry
  - Pins: `crates/interface-runtime/tests/session_management.rs::a_registry_failure_after_the_provider_fork_is_outcome_unknown`
- **[internal]** `a57aae4` · 2026-11-14T22:38:53-06:00 · **Only the app shell talks to the backend, now checked by a test** (`internal/window-iface-rule`)
  - Notes: Only App.tsx and main.tsx reach the host from the renderer, and a test now says so
- **[speed]** `cd5e5ca` · 2026-11-14T22:33:30-06:00 · **The sidebar and hidden terminal screen stay still while a chat streams** (`speed/stable-thread-callbacks`)
  - Notes: While a chat streams, the sidebar and the hidden Code screen stop redrawing every frame (196 redraws in 5 s became 1 and 0)
  - Pins: `electron/src/renderer/app-memo-props.test.ts::keeps the thread callbacks stable: no dependency array names the whole `threads` handle`, `electron/src/renderer/app-memo-props.test.ts::passes <OutputPreviewPanel> no inline callbacks or literals`
- **[speed]** `5d772f0` · 2026-11-14T22:27:42-06:00 · **Threads panes skip two Chat-only passes while streaming** (`speed/threads-skip-chat-summaries`)
  - Notes: No visible change; a streaming Threads pane does about a tenth less JavaScript work per word
  - Pins: `electron/src/renderer/thread-layout-passes.test.ts::does not work out the Chat chip or the Chat Fork's turn ends`
- **[internal]** `c336d28` · 2026-11-14T22:19:31-06:00 · **Thread line totals use one rule; two unused leftovers removed** (`internal/renderer-leftovers`)
  - Notes: The thread pane head and inbox row total a thread's lines by one rule, and two leftovers are gone
- **[fix]** `b483cc5` · 2026-11-14T22:14:35-06:00 · **Declined edits no longer count as changes in Chat** (`fix/declined-edits-not-counted`)
  - Notes: Deny an edit in a Chat pane: no '1 file changed' pill, turn chip, sidebar line count or recap claim for it
  - Pins: `electron/src/renderer/diff-stats.test.ts::leaves a session whose only edit was declined with no stat and no paths`, `electron/src/renderer/conversation.test.ts::omits the pill when the only edit was declined`
- **[fix]** `45c4529` · 2026-11-10T11:30:04-06:00 · **Fix pre-promote worktree, hook, startup, approval, queue-edit, search, and Codex reopen bugs** (`fix/pre-promote-regressions-20261110`)
  - Notes: Deleted checkouts prune cleanly, Shell panes do not stall Muse, launch failure shows one alert, multi-approvals stay Waiting, queue edits and ⌘F behave
  - Pins: `crates/interface-registry/src/worktree.rs::deleted_or_forced_ignored_checkout_removes_cleanly`, `apps/interface-host/src/native_tests.rs::cleanup_removes_deleted_checkout_without_verifying_missing_directory`, `apps/interface-host/src/worktree_release_tests.rs::deleted_checkout_is_released_even_when_verification_would_fail`, `apps/interface-host/src/provider_capture/native.rs::lineage_hook_does_not_wait_for_open_manual_shell_panes`, `electron/src/main/main-wiring.test.ts::destroys an already-created window and shuts down durably on top-level startup rejection`, `crates/engine-claude/src/adapter_tests.rs::concurrent_approvals_stay_waiting_until_last_request_is_settled_or_cancelled`, `crates/engine-codex/src/adapter_tests.rs::concurrent_approvals_stay_waiting_until_last_approval_is_confirmed`, `crates/engine-acp/src/adapter_tests.rs::concurrent_approvals_stay_waiting_until_last_permission_is_confirmed`, `electron/src/renderer/components/Conversation.queue.test.ts::releases session A's queue-edit lock when switching sessionKey A -> B`, `electron/src/renderer/outbound-lane.test.ts::clears queueEditing when a session is dropped so a recreated key is never wedged`, `electron/src/renderer/outbound-lane.test.ts::refuses editing a queued prompt into a slash command`, `electron/src/renderer/terminal-keys.test.ts::refuses ⌃⌘F, ⇧⌘F, ⌥⌘F and IME composition on macOS, and uses Ctrl+Shift+F elsewhere`, `crates/interface-settings/src/terminal_workspaces.rs::stale_codex_roster_cannot_undo_a_host_persisted_shell_demotion`
- **[fix]** `0c08aaa` · 2026-11-10T08:29:45-06:00 · **One alert when the backend fails while Interface is opening** (`fix/startup-failure-single-alert`)
  - Notes: A backend that fails during launch shows only 'Interface could not open'; a later crash still offers Relaunch
  - Pins: `electron/src/main/main-wiring.test.ts::shows only the startup failure when the backend fails before it has started`
- **[fix]** `2f25234` · 2026-11-10T08:28:08-06:00 · **A queued follow-up being edited is not sent until you save or cancel** (`fix/queued-edit-survives-turn-end`)
  - Notes: Queue a follow-up, click Edit, let the agent finish: nothing is sent and your edit stays; Save sends the new wording
  - Pins: `electron/src/renderer/outbound-lane.test.ts::does not deliver a queued prompt open for editing when the turn ends`
- **[fix]** `3ca7edb` · 2026-11-10T08:26:20-06:00 · **Muse hooks check once and exit unless a pane of this channel is still starting** (`fix/muse-hook-lineage-wait`)
  - Notes: Muse Code no longer pauses about a second on every tool call, inside or outside Interface
  - Pins: `apps/interface-host/src/provider_capture/native.rs::lineage_hook_waits_only_while_a_process_is_not_yet_filed`
- **[fix]** `a2d4d0b` · 2026-11-10T08:24:24-06:00 · **A Kimi Code pane that starts a new chat reopens after a restart** (`fix/kimi-pane-reopen`)
  - Notes: In a Kimi pane use /new, then restart Interface: the pane comes back on the new chat
  - Pins: `apps/interface-host/src/bootstrap_tests.rs::every_captured_provider_session_change_reaches_the_saved_roster`
- **[fix]** `40d8c72` · 2026-11-10T08:22:39-06:00 · **A Claude session shows as working again after you approve its request** (`fix/claude-working-after-approval`)
  - Notes: Approve a Claude tool request: the Dock badge stops counting it as waiting and the Mac stays awake while it works
  - Pins: `crates/engine-claude/src/adapter_tests.rs::confirmed_approval_puts_the_session_back_to_working`
- **[fix]** `3ca1f37` · 2026-11-10T08:20:41-06:00 · **Ending an isolated session no longer deletes git-ignored files or unregisters other worktrees** (`fix/registry-worktree-keeps-ignored`)
  - Notes: End a session whose worktree has a .env or node_modules: the folder stays and a notice says it was kept
  - Pins: `crates/interface-registry/src/worktree.rs::removal_keeps_ignored_files_and_other_absent_worktrees`, `apps/interface-host/src/worktree_release_tests.rs::checkout_with_ignored_files_is_retained_once_with_a_notice_naming_them`
- **[fix]** `67ed686` · 2026-11-10T08:15:59-06:00 · **Ctrl+F in a terminal pane reaches the program; only ⌘F opens search** (`fix/terminal-ctrl-f`)
  - Notes: In a terminal pane, Ctrl+F moves the cursor forward in the shell; ⌘F still opens the search box
  - Pins: `electron/src/renderer/terminal-keys.test.ts::opens the terminal search box on ⌘F and leaves Ctrl+F to the program`
- **[record]** `0937757` · 2026-11-09T14:09:15-06:00 · **Agent rules: subagents hand work back, and xtask may relaunch Dev** (`rules/subagent-closeout`)

### v0.3.176 — 2026-10-28T22:05:31-05:00 (`9fa7e0a`)

- **[look]** `4f9958c` · 2026-10-28T18:39:18-05:00 · **The sidebar's working indicators keep moving while an agent works** (`look/ticker-stepped-20261028`)
  - Notes: Start an agent: the sidebar ticker and folder dot keep moving for the whole run, and still stop while another app is in front
  - Pins: `electron/src/renderer/loop-rest.test.ts::rests only loops outside the sidebar and leaves the sidebar's to their rules`

### v0.3.175 — 2026-10-27T16:26:45-05:00 (`d25768f`)

- **[look]** `f6739e4` · 2026-10-27T16:24:37-05:00 · **Working marks animate for a few seconds, then hold still, so busy agents stop redrawing nonstop** (`look/loops-rest-20261027`)
  - Notes: Start an agent: the sidebar ticker moves for about 3 s, then holds still; a new turn moves it again
  - Pins: `electron/src/renderer/loop-rest.test.ts::lets a loop run about three seconds, then holds the first frame of its cycle`
- **[speed]** `dec9d2c` · 2026-10-27T16:18:27-05:00 · **Agents working in background panes use far less battery while another app is in front** (`speed/battery-background-20261027`)
  - Notes: With two or three agents working and another app in front, Interface's renderer and GPU drop from about 25% CPU to under 8%
- **[fix]** `f740bff` · 2026-10-27T12:35:27-05:00 · **Shift+Tab stays in its terminal pane** (`fix/shift-tab-terminal-pane-20261027`)
  - Notes: In a terminal pane, Shift+Tab cycles the model's permission mode and focus stays in that pane
  - Pins: `electron/src/renderer/terminal-keys.test.ts::claims plain Shift+Tab so it reaches the program instead of moving pane focus`, `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps Shift+Tab in the pane as the program's reverse tab, never a focus move`
- **[record]** `fc72ef1` · 2026-10-26T21:13:12-05:00 · **Record that Kimi Code shipped in 0.3.174** (`record/kimi-shipped-20261026`)

### v0.3.174 — 2026-10-26T21:11:05-05:00 (`7cc8cfb`)

- **[internal]** `956b1cd` · 2026-10-26T21:10:31-05:00 · **The packaged smoke fakes kimi so release packaging sees the full launcher** (`fix/packaged-smoke-fake-kimi-20261026`)
  - Notes: Fake kimi in the packaged smoke sandbox
- **[record]** `26fecc2` · 2026-10-26T21:03:44-05:00 · **Record the Kimi follow-ups landing** (`feat/kimi-code-followups-20261026`)
  - Notes: Record the Kimi follow-ups landing in the TODO entry
- **[feature]** `29e3368` · 2026-10-26T21:02:55-05:00 · **Kimi Code panes reopen their chat, show working and done, accept Pass off and get their phone mark** (`feat/kimi-code-followups-20261026`)
  - Notes: In a Kimi pane send a prompt and see working then done, restart Dev and it reopens the same chat, and Pass off into Kimi types the brief (needs kimi installed)
  - Pins: `crates/interface-terminal/src/native_checkpoint.rs::kimi_binds_its_session_and_ends_a_turn_only_when_no_subagent_is_open`, `apps/interface-host/src/terminal_tests.rs::native_reopen_resumes_the_exact_kimi_session`, `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_kimi_pass_off_is_typed_once_after_its_editor_takes_a_paste`, `apps/interface-host/src/native_checkpoint_setup.rs::kimi_plugin_registers_every_observed_event_and_keeps_owner_plugins`
- **[record]** `f8155f5` · 2026-10-26T19:39:16-05:00 · **Note the Dev-app check for Muse stop detection** (`fix/muse-plugin-hooks-20261026`)
  - Notes: Note the owner check for Muse stop detection after /clear
- **[feature]** `eb4ff45` · 2026-10-23T14:44:55-05:00 · **Kimi Code is available in the terminal launcher and + menu** (`feat/kimi-code-terminal-20261023`)
  - Notes: New pane → Kimi Code opens kimi in the chosen folder with the Kimi mark (needs kimi installed)
  - Pins: `crates/interface-contracts/src/terminal.rs::kimi_wire_round_trips_the_shared_protocol_v54_fixture`, `apps/interface-host/src/bootstrap_tests.rs::kimi_discovery_uses_the_installer_binary_but_skips_non_executables`, `electron/src/renderer/components/TerminalLauncher.test.ts::offers Kimi Code with its own mark beside the other agents`

### v0.3.172 — 2026-10-26T20:25:42-05:00 (`6539916`)

- **[fix]** `5025b85` · 2026-10-26T19:37:05-05:00 · **Muse panes notice stops again after /clear** (`fix/muse-plugin-hooks-20261026`)
  - Notes: In a Muse pane, /clear then let it finish: the pane still switches to done and pings you
  - Pins: `apps/interface-host/src/provider_capture/native.rs::lineage_scan_selects_the_generation_whose_process_matches`, `apps/interface-host/src/native_checkpoint_setup.rs::muse_plugin_bundle_routes_every_event_by_lineage`
- **Left on Dev:** b57f0f0 Kimi Code is available in the terminal launcher and + menu

### v0.3.171 — 2026-10-23T14:28:52-05:00 (`bcc9f48`)

- **[fix]** `f5ed798` · 2026-10-23T14:25:44-05:00 · **Panes in a hidden workspace keep the chosen terminal text size** (`fix/terminal-text-size-parked-20261023`)
  - Notes: Every workspace shows the terminal text size chosen in Settings, including after a theme or light/dark change made while it was out of view
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::repaints a pane parked through an appearance change on its return, never from the parking container`

### v0.3.170 — 2026-10-23T14:08:58-05:00 (`1f05f9b`)

- **[record]** `af60122` · 2026-10-23T14:06:11-05:00 · **To-do: the Threads look's open canvas details and its live check** (`look/threads-claude-app-20261023`)
  - Notes: To-do: the Threads look's landing and the canvas details still open
- **[look]** `2b501d2` · 2026-10-23T14:04:42-05:00 · **Threads take the Claude app's Code mode look: new icons, the Pair working mark, roomier spacing** (`look/threads-claude-app-20261023`)
  - Notes: Open a thread: line icons, two beads cross while it works, no project glow, a roomier lane and inbox, Send in the theme accent
  - Pins: `electron/src/renderer/components/ThreadGlyph.test.ts::draws every icon on the one 16px grid with a 1.5 round stroke, hidden from assistive tech`, `electron/src/renderer/components/ThreadGlyph.test.ts::moves at the app's stepped rate, rests in the background and holds under reduced motion`, `electron/src/renderer/thread-view.test.ts::draws the status line under the trail, with the Pair mark and never the Wave`, `electron/src/renderer/components/ThreadInbox.test.ts::keeps the project's hue to its square: none on the row, its selection, its unread dot or the header`, `electron/src/renderer/thread-hue.test.ts::reads no hue in the transcript or the composer: the theme's own accents draw them`
- **[look]** `32c8e1b` · 2026-10-23T10:23:55-05:00 · **Light terminal screens hold text to 7:1 so status lines and CLI colours stay readable** (`fix/light-screen-contrast-floor-20261023`)
  - Notes: On Clay Light, Claude's status line (model, effort, tokens) and 'auto mode on' read in deep rust, forest and amber instead of faint orange and yellow
  - Pins: `electron/src/renderer/terminal-palettes.test.ts::holds text to 7:1 on light screens and leaves dark screens alone`
- **[fix]** `d1aa18b` · 2026-10-23T08:58:12-05:00 · **Markdown task lists draw checkboxes and indented lists nest under their item** (`fix/markdown-task-nested-lists-20261023`)
  - Notes: Open a Markdown file or reply with '- [x] done' and a bullet indented under a numbered item: boxes instead of [x], and the bullet sits under its item
  - Pins: `electron/src/renderer/markdown.test.ts::draws task items as read-only boxes and nests indented lists under their item`
- **[look]** `6db451c` · 2026-10-23T08:53:04-05:00 · **File viewer looks like the rest of the app: pane head band, tab row, composer-style feedback** (`look/output-viewer-20261023`)
  - Notes: Click a file link in a pane: the viewer's head lines up with the pane heads, open files are tabs in it, and feedback is a Threads-style box
  - Pins: `electron/src/renderer/components/OutputPreviewPanel.test.ts::draws the panes' head band, ghost actions and the app's own pickers`

### v0.3.169 — 2026-10-22T12:29:41-05:00 (`48724c6`)

- **[speed]** `9fe35be` · 2026-10-22T12:29:00-05:00 · **Interface uses far less battery: animations rest when it is in the background** (`speed/battery-animations-20261022`)
  - Notes: With an agent working and an update waiting, Interface's CPU drops to about 1% when another app is in front
  - Pins: `electron/src/renderer/window-activity.test.ts::steps every loop at about 12 frames a second and rests it while Interface is not in front`
- **[fix]** `202078f` · 2026-10-22T12:07:22-05:00 · **Fix Antigravity completion and permission indicators** (`fix/agy-completion-20261022`)
  - Notes: Antigravity panes show Done when AGY finishes and keep permission waits visible despite capture hooks.
  - Pins: `crates/interface-terminal/src/service/workspace_sleep_fixture_tests.rs::agy_native_status_overrides_hooks_for_attention_and_workspace_sleep`, `electron/src/renderer/terminal-activity.test.ts::follows native AGY status despite interleaved hooks at every byte boundary`

### v0.3.168 — 2026-10-22T10:38:55-05:00 (`bdb3d46`)

- **[feature]** `e0214c9` · 2026-10-22T09:58:40-05:00 · **Open clickable AI response file links in the in-app file viewer** (`feat/clickable-response-files`)
  - Notes: Click a file link or path in a Code terminal (including OMP) or a Threads/Agent Markdown response to open it in the side-by-side file viewer
  - Pins: `apps/interface-host/src/terminal_tests.rs::preview_output_reads_agent_session_artifacts_without_granting_directory_listing`, `electron/src/renderer/terminal-links.test.ts::resolves OSC 8 file:// URLs with query or line fragments to clean local file paths`, `electron/src/renderer/markdown.test.ts::turns local file links into preview actions when an onOpenFile handler is provided`, `electron/src/renderer/prompt-images-ui.test.ts::turns local file links in assistant responses into clickable preview links`

### v0.3.167 — 2026-10-21T16:57:36-05:00 (`2a98080`)

- **[feature]** `8be49c5` · 2026-10-21T16:54:33-05:00 · **Add full update log in docs/UPDATES.md and Settings -> Updates** (`feat/update-log`)
  - Notes: Settings -> Updates shows the searchable release and Dev landing update log, and docs/UPDATES.md stays synced on closeout and promote
  - Pins: `xtask/src/update_log.rs::parse_and_render_update_log_links_releases_and_landings`, `xtask/src/closeout.rs::closeout_keeps_docs_updates_md_in_sync_across_sequential_and_concurrent_landings`, `electron/src/renderer/update-log.test.ts::parses releases, release items, landings, notes, pins, and completion timestamps`, `electron/src/renderer/components/SettingsPage.test.ts::shows a controlled update center without adding chrome when updater state is absent`
- **[fix]** `53951df` · 2026-10-21T14:48:47-05:00 · **Harden terminal pass-off across MCP tools, host dispatch, and workspace seating** (`fix/pass-off-review`)
  - Notes: Passed-off panes launch for every profile, Codex delegates report back without approval prompts, and background pass-offs stay in their source workspace.
  - Pins: `apps/interface-host/src/pane_mcp.rs::pass_off_builds_a_brief_create_at_the_callers_cwd_pointing_back_at_it`, `apps/interface-host/src/terminal_tests.rs::send_to_pane_rejects_an_exited_pane`, `apps/interface-host/src/bootstrap_tests.rs::codex_pane_capture_argv_keeps_its_exact_order`

### v0.3.166 — 2026-10-21T09:17:39-05:00 (`8f11382`)

- **[feature]** `6523980` · 2026-10-21T08:52:32-05:00 · **Name Antigravity panes from the conversation title** (`feat/antigravity-pane-titles`)
  - Notes: An Antigravity pane adopts its generated conversation title or first prompt instead of staying named Antigravity
  - Pins: `crates/interface-terminal/tests/suite/embedded_terminal.rs::antigravity_panes_adopt_their_conversation_title_and_preserve_manual_renames`, `crates/interface-terminal/src/antigravity_names.rs::generated_annotation_replaces_the_first_request_and_ignores_metadata`

### v0.3.165 — 2026-10-21T07:42:39-05:00 (`90673e3`)

- **[fix]** `1ef3536` · 2026-10-21T07:37:27-05:00 · **Antigravity, Muse and OpenCode panes reopen after they switch conversations** (`fix/native-provider-reopen-identity`)
  - Notes: An Antigravity pane that started a new conversation comes back on that conversation after restarting Interface, instead of a failed tab
  - Pins: `apps/interface-host/src/bootstrap_tests.rs::changed_native_provider_session_reaches_the_saved_roster`
- **[feature]** `7fb692e` · 2026-10-21T07:31:50-05:00 · **Remove Halt all and update; Restart to update installs directly** (`feat/remove-halt-all-20261021`)
  - Notes: Restart to update shows a plain confirm and restarts; no pane is sent a stop request
  - Pins: `apps/interface-host/src/updater.rs::restart_to_update_installs_without_a_session_checkpoint`, `electron/src/renderer/update-restart-ui.test.ts::requires explicit confirmation before mutating Rust updater state`, `crates/interface-terminal/src/service/workspace_sleep.rs::agy_osc7501_tracks_turns_and_permission_waits`, `apps/interface-host/src/terminal_tests.rs::native_reopen_resumes_the_exact_agy_and_muse_session`
- **[feature]** `d1ec1bf` · 2026-10-20T22:41:57-05:00 · **Preview file and image attachments in-app** (`feat/in-app-attachment-preview`)
  - Notes: Click an attached file or image in the composer or a sent message to inspect it in the side-by-side preview panel with image zoom and lightbox
  - Pins: `crates/interface-workspace/src/output_preview_tests.rs::exact_file_grant_reads_only_that_attachment_outside_project_roots`, `electron/src/renderer/output-preview.test.ts::resolves in-project and external attachment preview targets`

### v0.3.164 — 2026-10-20T21:14:42-05:00 (`3b37ef4`)

- **[speed]** `b23c3d6` · 2026-10-20T20:49:06-05:00 · **Speed up release packaging and update staging** (`speed/promote-packaging-20261020`)
  - Notes: Release packaging compiles with parallel codegen, caches iface lib on version bumps, signs /private/tmp apps in place, and skips the redundant DMG target
  - Pins: `xtask/src/release.rs::selected_update_artifacts_accepts_zip_only_manifest`, `electron/scripts/sign-mac.test.cjs::path remapping adds timestamp none only to the local development build`

### v0.3.163 — 2026-10-20T20:19:14-05:00 (`97be60a`)

- **[fix]** `d36e1b7` · 2026-10-20T20:12:53-05:00 · **Mark Muse StopFailure and Interrupt hooks idle** (`fix/muse-stop-failure-activity-20261020`)
  - Notes: A Muse Code pane that stops on a usage limit, error or interrupt returns to idle instead of staying active
  - Pins: `crates/interface-terminal/src/native_checkpoint.rs::muse_stop_failure_and_interrupt_release_to_idle_and_ignore_post_stop_noise`

### v0.3.162 — 2026-10-20T19:20:17-05:00 (`d224112`)

- **[fix]** `6900923` · 2026-10-20T18:36:11-05:00 · **Fix Claude worktree receipt reconciliation and unsaved shell reopen authority** (`fix/terminal-reopen-capture-and-authority`)
  - Notes: Claude panes that used a worktree and newly spawned shells reopen cleanly after restart
  - Pins: `apps/interface-host/src/claude_terminal_session_tests.rs::owned_capture_adopts_and_reconciles_a_worktree_clear_receipt_and_allows_reopen`, `crates/interface-desktop-services/src/service_tests.rs::in_flight_roster_save_before_filing_does_not_strip_newly_spawned_reopen_authority`

### v0.3.161 — 2026-10-20T17:29:40-05:00 (`ad76ef6`)

- **[fix]** `4125d94` · 2026-10-20T17:08:10-05:00 · **Antigravity panes show working, waiting, and done indicators** (`fix/agy-working-indicator-20261020`)
  - Notes: Antigravity panes show the working indicator during turns, wait on approvals, and mark finished turns done
  - Pins: `electron/src/renderer/terminal-activity.test.ts::tracks working, blocked, done, error, and clear across Antigravity and shell panes`, `crates/interface-terminal/src/service/workspace_sleep.rs::agy_osc7501_tracks_turns_and_preserves_mid_turn_checkpoint_markers`
- **[fix]** `5a6615f` · 2026-10-20T17:01:27-05:00 · **Fix GUI review bugs, left-behind panels, and paper cuts** (`fix/gui-review-bugs-20261020`)
  - Notes: Primary CTA hover/disabled contrast, From terminals inbox shelf actions and paging, Away loop composer lock, and left-behind panel polish work cleanly
  - Pins: `electron/src/renderer/thread-inbox.test.ts::pages From terminals independently of Settled`, `electron/src/renderer/away-thread.test.ts::locks the composer and turn actions while the Away loop is running`
- **[speed]** `8c7206b` · 2026-10-20T16:51:57-05:00 · **Cache opencode version probes, reduce ThreadHost lock/view work, and trim git and Claude scans** (`speed/wave2-review-20261020`)
  - Notes: Usage polling, thread background ticks, commit review diffs, and Claude session listings run with fewer subprocesses and lock holds
  - Pins: `crates/engine-acp/src/opencode_usage_tests.rs::version_probe_is_cached_across_usage_reads_until_executable_changes`, `crates/interface-review/src/thread_ship_tests.rs::branch_state_resolves_branch_remote_and_default_without_scanning_working_tree`
- **[speed]** `293071f` · 2026-10-20T16:33:48-05:00 · **Keep terminal/thread props stable, gate hidden overview timers, and refresh repeated EOF keys** (`speed/wave1-review-20261020`)
  - Notes: Switching surfaces or streaming turns keeps idle panes and hidden overviews quiet
  - Pins: `electron/src/renderer/app-memo-props.test.ts::keeps outputFeedbackTargets and withShip references stable when visible fields do not change`, `electron/src/main/terminal-data-transport.test.ts::refreshes an already-ended stream key without evicting it prematurely when the window rolls`
- **[feature]** `1ef2083` · 2026-10-20T16:14:47-05:00 · **Undo a worktree thread's turn files from Changes** (`feat/threads-leftovers-20261020`)
  - Notes: In a worktree thread's Changes, a turn's Undo restores its files (recovery point kept); the conversation is unchanged.
  - Pins: `electron/src/renderer/thread-view.test.ts::restores a worktree turn's files only, saying where the recovery point is`, `electron/src/renderer/components/ThreadDiffPanel.test.ts::offers Undo turn on a turn of a local or owned-worktree thread, never a shared worktree's`
- **[feature]** `0d97b51` · 2026-10-20T16:13:02-05:00 · **Hidden sidebar comes back on hovering its toggle beside the traffic lights** (`sidebar-toggle-peek`)
  - Notes: Hide the sidebar, rest the pointer on the toggle by the traffic lights: the panel slides out below it and leaves when you move away; the left edge does nothing
  - Pins: `electron/src/renderer/sidebar-peek.test.ts::slides the hidden sidebar out after the pointer rests on its toggle, and away once it leaves`, `electron/src/renderer/sidebar-peek.test.ts::does nothing while the sidebar is docked, so hiding it under the pointer never brings it straight back`
- **[record]** `388b216` · 2026-10-20T15:55:16-05:00 · **TODO: relay allowlist set, phone rebuilt** (`record/phone-path-20261020`)
  - Notes: TODO: relay allowlist set, phone rebuilt, install awaits the phone
- **[speed]** `7beb9b7` · 2026-10-20T15:39:57-05:00 · **Dev app parses JSON about twice as fast** (`speed/deps-opt-level-20261020`)
  - Notes: Interface Dev runs optimised dependencies: JSON about twice as fast, crypto about four times. First build after this rebuilds dependencies once.
- **[feature]** `2ff0a90` · 2026-10-20T15:27:48-05:00 · **Antigravity and Muse pause and resume for updates** (`feat/update-halt-agy-muse-20261020`)
  - Notes: New Antigravity and Muse panes find stopping points before updating, then resume the same chats once.
  - Pins: `apps/interface-host/src/terminal_tests.rs::native_update_resume_reopens_agy_and_muse_and_sends_one_continuation`, `crates/interface-terminal/src/muse_resume.rs::muse_resume_waits_for_exact_process_session_and_input_ready_without_a_hook`
- **[fix]** `3945ce0` · 2026-10-20T14:59:09-05:00 · **Installed Interface stops re-copying a staged update** (`fix/updater-no-recopy-20261020`)
  - Notes: With an update staged, update.zip in ~/Library/Caches/interface-electron-updater is no longer rewritten every few minutes
  - Pins: `electron/src/main/native-updater.test.ts::never re-copies a staged update into the differential cache`
- **[fix]** `4f7c8c0` · 2026-10-20T14:57:39-05:00 · **Antigravity usage read no longer opens Google sign-in in Chrome** (`fix/agy-usage-browser-login-20261020`)
  - Notes: No new Chrome tabs ask you to log into Google Antigravity; Gemini quota reads as before when signed in
  - Pins: `apps/interface-host/src/antigravity_usage.rs::the_childs_browser_openers_are_no_ops`, `apps/interface-host/src/antigravity_usage.rs::a_failed_read_backs_off_instead_of_respawning`
- **[feature]** `e6bfa12` · 2026-10-20T14:52:24-05:00 · **Hidden sidebar comes back when the pointer rests on the left edge** (`sidebar-edge-peek`)
  - Notes: Hide the sidebar, then rest the pointer on the window's left edge: it slides back over the panes and leaves when you move away; the toggle keeps it
  - Pins: `electron/src/renderer/sidebar-peek.test.ts::slides the hidden sidebar out after the pointer rests on the left edge, and away once it leaves`
- **[record]** `2297fa9` · 2026-10-20T14:51:44-05:00 · **TODO: sprintf, lead-2/lead-8 and Computer-note rechecks** (`record/easy-items-recheck-20261020`)
  - Notes: TODO: recheck sprintf, lead-2/lead-8 and the Computer tab note
- **[internal]** `0cdf475` · 2026-10-20T14:43:21-05:00 · **Remove the retired OpenCode native worker view** (`chore/drop-opencode-native-view-20261020`)
  - Notes: Remove the OpenCode native-view session code; Drop the native path from the OpenCode worker runner
- **[feature]** `fc5d069` · 2026-10-20T14:40:05-05:00 · **Models pause for updates and resume their saved work** (`feat/update-halt-resume-20261020`)
  - Notes: Halt all and update asks working models for a handoff, waits for every stop, then resumes the same sessions after restart.
  - Pins: `crates/interface-updater/src/sessions.rs::all_models_must_checkpoint_before_install_and_failed_install_can_resume_them`, `apps/interface-host/src/terminal_tests.rs::update_resume_reopens_the_exact_saved_provider_session_with_one_continuation`
- **[fix]** `929a07c` · 2026-10-20T12:49:27-05:00 · **Gate passes from an armed, busy Interface pane** (`fix/overlay-tests-pane-env-20261020`)
  - Notes: Running the test gate from inside Interface on a busy machine no longer fails the shell-overlay and terminal tests.
  - Pins: `apps/interface-host/src/bootstrap_tests.rs::manual_codex_overlay_adds_titles_and_preserves_user_overrides`, `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_pass_off_into_a_live_subdirectory_keeps_its_root_and_restarts`
- **[record]** `4959405` · 2026-10-20T09:06:44-05:00 · **TODO: Muse Code plan in the usage tracker is deferred** (`record/muse-usage-deferred`)

### v0.3.160 — 2026-10-20T08:58:34-05:00 (`bcead5f`)

- **[look]** `c842b93` · 2026-10-20T08:58:09-05:00 · **Muse Code shows its mark on the iPhone** (`look/phone-muse-mark-20261020`)
  - Notes: Muse Code sessions on the phone show the blue Muse squiggle instead of a terminal glyph.
  - Pins: `apps/interface-ios/MacAddressTests.swift::testMuseCodeSessionsShowTheMuseMarkInMuseBlue`
- **[look]** `301540f` · 2026-10-20T08:47:02-05:00 · **Muse Code shows in Muse blue** (`look/muse-blue-20261020`)
  - Notes: The Muse Code squiggle and its sidebar pips are blue: deep blue on light themes, brighter blue on dark.
  - Pins: `electron/src/renderer/components/ProviderLogo.test.ts::draws Muse Code with the official Muse squiggle in Muse blue`

### v0.3.159 — 2026-10-19T21:20:20-05:00 (`a2af762`)

- **[look]** `7a98951` · 2026-10-18T09:23:58-05:00 · **Muse Code uses its official logo** (`fix/muse-official-logo-20261018`)
  - Notes: Muse Code shows the official Muse squiggle in the launcher and pane headers.
  - Pins: `electron/src/renderer/components/ProviderLogo.test.ts::draws Muse Code with the official Muse squiggle and keeps its accent neutral`
- **[record]** `495a344` · 2026-10-15T14:45:32-05:00 · **TODO: the 14 Oct review work shipped in 0.3.158** (`record/ship-0-3-158`)

### v0.3.158 — 2026-10-15T14:40:34-05:00 (`802f490`)

- **[feature]** `c40ed87` · 2026-10-15T09:48:57-05:00 · **Provider CLI drift canary: Settings says what changed in a CLI since its last check** (`feat/gate-tooling-20261015`)
  - Notes: After a CLI updates, Settings → Models & providers → Terminal tools says "Changed since last check: old → new"; iface doctor providers lists every CLI
  - Pins: `crates/interface-terminal/src/provider_canary_tests.rs::provider_canary_reports_a_matching_cli_and_names_each_drifted_flag_and_choice`, `crates/interface-terminal/src/provider_canary_tests.rs::provider_canary_runs_only_version_help_listing_and_schema_commands`, `apps/interface-host/src/terminal_tests.rs::settings_terminal_tools_say_what_changed_in_a_cli_since_the_last_provider_check`, `electron/src/renderer/terminal-tools.test.ts::names the versions and what no longer matches, and says nothing without a change`
- **[internal]** `edbc5a7` · 2026-10-15T09:27:53-05:00 · **Packaged smoke stops pinning look values; the DOM gate checks the pane grid against its tokens** (`feat/gate-tooling-20261015`)
  - Notes: Packaged smoke stops pinning look values; the DOM gate checks the grid against its tokens
- **[internal]** `b3c6cfc` · 2026-10-15T09:19:50-05:00 · **Cold-start restore matrix: every saved pane reopens or says why after a restart** (`feat/gate-tooling-20261015`)
  - Notes: Cold-start restore matrix: every saved pane reopens or says why, never blank
- **[record]** `9c9acd1` · 2026-10-15T08:48:29-05:00 · **Backlog: closed entries moved to done, status lines corrected, one owner decision sheet** (`record/backlog-triage-20261015`)
- **[fix]** `170ce2e` · 2026-10-15T08:41:56-05:00 · **Review says git timed out (iCloud may be downloading), not missing; project search has a deadline** (`feat/gate-tooling-20261015`)
  - Notes: Review on an iCloud project whose git stalls says 'Git did not finish … within 15 seconds' and names iCloud
  - Pins: `crates/interface-review/tests/suite/repository_review.rs::review_says_git_timed_out_and_names_icloud_instead_of_calling_git_missing`, `crates/interface-workspace/src/search.rs::project_search_says_a_project_still_downloading_from_icloud_instead_of_hanging`
- **[feature]** `917572c` · 2026-10-15T08:33:07-05:00 · **Second opinion and Continue with in a thread; Threads previews; plan headroom** (`feat/threads-second-opinion-20261014`)
  - Notes: Thread ⋯: Second opinion from Codex (a card above the composer) and Continue with; Preview opens a changed PDF/Markdown beside it; chips say % left
  - Pins: `electron/src/renderer/components/ThreadOpinionCards.test.ts::is the other provider's card, never a turn: attributed, held until sent on`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::shows what goes out before anything leaves: Ask or Cancel, with the packet`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::offers Continue with when the thread's provider hit its limit, never by itself`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::puts Second opinion and Continue with in the pane menu, waiting while a turn runs`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::offers Preview on a changes card's previewable files`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::lets Quick Open open the highlighted PDF, image or Markdown in the preview`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::says it quietly on the model chip, in its accessible name and tooltip`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::puts it under an agent tile's name in the launcher`, `electron/src/renderer/thread-opinion.test.ts::draws a thread's consults as cards: review, running, the reply, sending, failed`, `electron/src/renderer/thread-opinion.test.ts::offers Continue with the provider with the most headroom when a thread hits its limit`, `electron/src/renderer/thread-opinion.test.ts::says what is left of the fullest window, quietly, and nothing for an unknown`
- **[fix]** `c22a3e9` · 2026-10-14T23:49:45-05:00 · **Project reads say a file is still downloading from iCloud instead of hanging** (`fix/security-robustness-20261014`)
  - Notes: In an iCloud project with evicted files, Quick Open, previews, Changes and Threads say it is still downloading within seconds
  - Pins: `crates/interface-common/src/bounded_read.rs::a_read_that_blocks_on_open_fails_at_its_deadline_and_a_quick_one_answers`, `crates/interface-review/src/thread_git_tests.rs::an_index_still_downloading_from_icloud_fails_clearly_at_its_deadline`, `crates/interface-review/tests/suite/repository_review.rs::untracked_files_still_downloading_from_icloud_are_omitted_at_their_deadline`, `crates/interface-workspace/src/search.rs::quick_open_says_a_project_still_downloading_from_icloud_instead_of_hanging`, `crates/interface-workspace/src/editor.rs::a_preview_read_still_downloading_from_icloud_says_so_at_its_deadline`
- **[fix]** `2924551` · 2026-10-14T23:36:55-05:00 · **An agent that finishes in a parked workspace gets its shell at once (Rust decides)** (`fix/security-robustness-20261014`)
  - Notes: Let Codex (or any agent) exit cleanly while its workspace is parked: its row and pane show a shell right away, not a dead tab
  - Pins: `apps/interface-host/src/terminal_tests.rs::an_agent_that_exits_by_itself_is_handed_a_shell_by_rust_whatever_the_window_shows`, `crates/interface-terminal/src/shell_handoff.rs::only_an_agent_that_exits_cleanly_is_handed_a_shell_where_it_worked`, `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalExitHandoffFixture`, `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalHandoffSwitchFixture`, `electron/src/renderer/terminal-shell-handoff.test.ts::moves a parked workspace's saved layout without touching the shown one`, `crates/interface-contracts/src/terminal.rs::a_handed_off_shell_names_the_agent_it_replaces_like_the_shared_fixture`
- **[record]** `342f106` · 2026-10-14T23:26:55-05:00 · **Record completed workflow acceptance and verified Dev landing** (`feat/workflow-completion-20261014`)
- **[feature]** `66d2ea9` · 2026-10-14T23:25:35-05:00 · **Complete reviewed workflows across desktop, phone and background runs** (`feat/workflow-completion-20261014`)
  - Notes: Choose commit hunks, reuse ready worktrees and recipes, review outputs, recover prompts, inspect tools, use the phone inbox, and run with the window closed
  - Pins: `crates/interface-review/src/thread_commit_tests.rs::selected_hunk_preserves_unselected_content_and_unrelated_staging`, `crates/interface-settings/src/workflows.rs::recipes_reopen_restore_context_and_require_explicit_use_before_setup_changes`, `crates/interface-settings/src/workflows.rs::setup_reserve_survives_restart_as_interrupted_without_replay`, `crates/interface-settings/src/output_reviews.rs::durable_reviews_keep_immutable_versions_and_refuse_lost_updates`, `crates/interface-settings/src/prompt_recovery.rs::recovery_never_replays_unknown_and_retains_original_context_after_restart`, `crates/interface-workspace/src/inventory_provider_tests.rs::mcp_check_initializes_and_pings_without_tools_or_raw_output`, `crates/interface-remote/src/threads.rs::inbox_includes_only_bound_agent_work_and_keeps_completed_tasks_read_only`, `crates/interface-settings/src/background.rs::background_choice_survives_restart_and_has_one_writer`
- **[fix]** `f5d59ff` · 2026-10-14T23:12:37-05:00 · **Grok and Antigravity get Interface's tools only when you switch them on** (`fix/security-robustness-20261014`)
  - Notes: Settings → Models & providers → Terminal tools: Grok and Antigravity have an off-by-default "use Interface's tools" switch
  - Pins: `apps/interface-host/src/terminal_tests.rs::settings_switches_grok_onto_interfaces_tools_only_when_asked_and_never_at_start`, `apps/interface-host/src/pane_mcp.rs::pane_tools_are_written_only_when_switched_on_and_only_interfaces_entry_is_removed`, `electron/src/renderer/terminal-tools.test.ts::shows Settings' switch off until the owner turns it on, and locks a taken or unreadable one`, `crates/interface-contracts/src/terminal.rs::pane_tools_states_and_changes_match_the_shared_fixture`
- **[feature]** `e4646a5` · 2026-10-14T23:00:22-05:00 · **Away reviews with the other vendor by default; the start form picks the reviewer** (`feat/away-reviewer-20261014`)
  - Notes: Away's start form shows Reviewed by: the other vendor when it can run; each review card in the thread names the provider that reviewed
  - Pins: `crates/interface-collaboration/src/project_away/tests.rs::a_review_runs_on_the_chosen_reviewer_with_its_own_restrictions`, `crates/interface-collaboration/src/project_away/tests.rs::a_v2_store_is_carried_over_and_its_loops_review_as_before`, `apps/interface-host/src/project_away.rs::a_review_session_starts_on_the_reviewers_provider`, `crates/interface-contracts/src/away.rs::away_reviewer_fixture_round_trips_and_defaults_to_the_worker`, `electron/src/renderer/away-view.test.ts::defaults to the other vendor when it can run, else the worker reviews`
- **[fix]** `12783e4` · 2026-10-14T22:46:51-05:00 · **Agents ask before every web fetch** (`fix/security-robustness-20261014`)
  - Notes: An agent in Ask mode asks you before any WebFetch (GET included); Away still never fetches; web search stays automatic
  - Pins: `crates/interface-desktop-services/src/agent_policy.rs::web_fetch_asks_the_owner_first_whatever_the_method_or_task`
- **[fix]** `1801448` · 2026-10-14T22:41:10-05:00 · **Threads avoid redundant reads and clean up background work** (`fix/performance-review-20261014`)
  - Notes: Threads retain the newest diff during rapid selections, stop queued reads when closed, and reuse unchanged thread rows.
  - Pins: `electron/src/renderer/latest-read.test.ts::a hundred selections run only the in-flight read and the newest pending read`, `electron/src/renderer/use-plan-usage.test.ts::disposal ignores a late answer and prevents its queued CLI read`, `crates/interface-settings/src/threads_tests.rs::view_checks_each_shared_worktree_once_and_refreshes_on_the_next_read`, `crates/interface-settings/src/threads_tests.rs::seeding_a_full_library_keeps_newest_first_across_order_key_spreads`
- **[look]** `e4ecd56` · 2026-10-14T22:36:15-05:00 · **The phone shows what needs you and the Mac's Threads** (`feat/phone-needs-you-look-20261014`)
  - Notes: On the iPhone, a session waiting for permission says Needs you and sorts first, and Code | Threads opens the Mac's Threads with Allow once and a follow-up field
  - Pins: `apps/interface-ios/MacAddressTests.swift::testSessionsThatNeedYouComeFirstAndLiftTheirProject`, `apps/interface-ios/MacAddressTests.swift::testAnUncertainFollowUpWaitsForANewerTurnOrTheOwnersCheck`
- **[record]** `edf8b86` · 2026-10-14T22:27:35-05:00 · **Record review D6: host and protocol landed, phone screens await look approval** (`record/phone-needs-you-20261014`)
  - Notes: Record review D6: host and protocol landed, the phone screens await look approval
- **[record]** `44f872d` · 2026-10-14T22:20:50-05:00 · **Record review B4, B5 and E: the backend landed, the renderer awaits look approval** (`docs/threads-b4-record-20261014`)
- **[internal]** `b2e9672` · 2026-10-14T22:17:23-05:00 · **The phone's Mac answers which sessions need you and serves Threads (phone screens await approval)** (`feat/phone-needs-you-20261014`)
  - Notes: The phone's Mac answers what needs you and serves Threads
  - Pins: `crates/interface-remote/src/threads.rs::the_phone_lists_threads_that_need_you_then_working_then_the_macs_order`, `crates/interface-remote/src/workspaces.rs::only_roster_v4_says_which_sessions_need_you`, `crates/interface-remote/src/http_threads_tests.rs::an_approval_answer_needs_the_lease_runs_once_and_never_repeats_an_unknown`, `crates/interface-remote/src/http_threads_tests.rs::a_follow_up_is_sent_once_under_the_lease_and_never_mistaken_for_an_answer`, `apps/interface-host/src/remote.rs::the_phone_answers_only_the_current_request_no_pane_holds`, `apps/interface-ios/MacAddressTests.swift::testThreadsUseTheSharedFixtureAndAddressOnlyWellFormedKeys`
- **[fix]** `4d2eff8` · 2026-10-14T22:09:48-05:00 · **A consult never writes to a session a Code pane holds; Second opinion and Continue with on the wire** (`feat/threads-b4-wire-20261014`)
  - Notes: From the Coordination canvas, a consult on a thread that is open in a Code pane is refused and nothing reaches that session
  - Pins: `apps/interface-host/src/collaboration_tests.rs::a_consult_never_writes_to_a_session_a_pane_holds`, `apps/interface-host/src/threads_tests.rs::second_opinion_resumes_the_thread_and_submits_one_consult_on_its_latest_turn`, `apps/interface-host/src/threads_tests.rs::second_opinion_refuses_a_held_session_a_running_turn_and_its_own_provider`, `apps/interface-host/src/threads_tests.rs::continue_with_prepares_the_threads_folder_and_binds_a_handoff_lineage`, `crates/interface-contracts/src/threads.rs::shared_v46_second_opinion_and_handoff_round_trip_and_reject_extra_fields`, `crates/interface-settings/src/threads_tests.rs::a_v2_store_is_carried_over_once_and_wins_over_v1`, `crates/interface-settings/src/threads_tests.rs::a_handoff_bind_records_its_lineage_and_only_v3_holds_one`, `crates/interface-collaboration/src/thread_handoff.rs::handoff_prompt_carries_the_changes_and_newest_conversation_inside_the_packet_bound`
- **[look]** `4331a8c` · 2026-10-14T22:07:57-05:00 · **An Away loop's thread wears Away's coffee in the Threads inbox** (`feat/away-inbox-label-20261014`)
  - Notes: Threads inbox: a row an Away loop runs as shows a small coffee glyph before its title
  - Pins: `electron/src/renderer/components/ThreadInbox.test.ts::carries Away's coffee before the title and says Away loop`, `electron/src/renderer/thread-inbox.test.ts::marks the row of a thread an Away loop runs as, and names it so`
- **[record]** `ba4a6a0` · 2026-10-14T22:02:34-05:00 · **Record Away loops as Threads (B3) and the inbox look awaiting approval** (`docs/away-threads-todo-20261014`)
  - Notes: Record Away loops as Threads (review B3) and its pending inbox look
- **[feature]** `d731888` · 2026-10-14T21:59:10-05:00 · **Away loops run as Threads: their own worktree, a turn per round, the review's verdict, the report** (`feat/away-threads-loop-20261014`)
  - Notes: Start Away: its thread opens in a new worktree, each round a turn with its diff and the review's verdict, the report at the end; the panel lists loops
  - Pins: `crates/interface-collaboration/src/project_away/tests.rs::a_loop_runs_as_one_thread_in_its_own_worktree_with_fresh_reviews`, `apps/interface-host/src/threads_tests.rs::an_away_loop_holds_its_thread_and_takes_only_its_own_prompts`, `apps/interface-host/tests/suite/project_away.rs::project_away_real_host_native_turn_review_and_retained_results`, `crates/interface-collaboration/src/project_away/tests.rs::a_v1_store_is_carried_over_once_and_left_unchanged`, `electron/src/renderer/away-thread.test.ts::puts each round's review under its turn, and the report at the end`, `crates/interface-contracts/src/threads.rs::shared_v45_away_thread_round_trips_and_rejects_extra_fields`, `apps/interface-host/src/dispatcher_tests.rs::a_host_prompt_is_shown_in_its_pane_before_the_send`
- **[feature]** `a272274` · 2026-10-14T21:49:38-05:00 · **Rust decides which terminal panes need you (desktop reads it; the phone can next)** (`feat/phone-needs-you-20261014`)
  - Notes: A pane asking for permission shows Needs attention as before, and still does after reloading the window
  - Pins: `crates/interface-terminal/src/attention.rs::terminal_attention_follows_the_shared_fixture`, `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_bell_makes_the_pane_need_you_until_a_keystroke_answers_it`, `electron/src/renderer/terminal-attention.test.ts::publishes the host's waiting and never its own`
- **[look]** `6aa54a3` · 2026-10-14T21:17:33-05:00 · **The launcher folds missing agents under More, with install hints** (`feat/launcher-fold-20261014`)
  - Notes: New workspace: missing agents other than Claude Code, Codex and Terminal fold under More with install hints and Refresh; Settings shows commands as code
  - Pins: `electron/src/renderer/components/TerminalLauncher.test.ts::folds tools that are not installed under More, each with its install hint`, `electron/src/renderer/components/SettingsPage.test.ts::shows a missing tool's install hint alone, its commands set as code`
- **[look]** `2d12e0b` · 2026-10-14T21:14:40-05:00 · **Live panes and a From terminals shelf in Threads** (`feat/one-product-looks2-20261014`)
  - Notes: Code's pane-card column and its ··· row say Live panes (tabs Panes, Agent tasks); Threads ends in a folded From terminals shelf
  - Pins: `electron/src/renderer/thread-inbox.test.ts::draws Rust's From terminals as the last shelf, folded, offering Continue as thread`, `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps the reference layout rows, now inside the window's ··· menu`, `electron/src/renderer/palette-commands.test.ts::reaches Away, voice, iPhone access, Files, Browser, overview, New project and themes (review C3)`
- **[fix]** `4f07365` · 2026-10-14T19:58:14-05:00 · **A thread whose provider fails at once shows Failed instead of thinking forever** (`feat/away-threads-20261014`)
  - Notes: Start a thread that fails at once (a signed-out provider): its turn shows Failed with Sign in, and no Thinking clock keeps running
  - Pins: `apps/interface-host/src/threads_tests.rs::a_launch_that_fails_before_its_bind_ends_turn_zero_failed`
- **[record]** `c7a7f12` · 2026-10-14T19:48:41-05:00 · **Record review C4: Terminal tools landed, the folded launcher awaits approval** (`docs/c4-todo-20261014`)
- **[record]** `80a7b1f` · 2026-10-14T19:37:47-05:00 · **Record the one-product work and its pending look approval** (`chore/one-product-todo-20261014`)
  - Notes: Record the one-product work (C1, C2, D8) and its pending look approval
- **[feature]** `3b77587` · 2026-10-14T19:37:24-05:00 · **Models & providers lists the launcher's terminal tools, with versions and install hints** (`feat/provider-tools-20261014`)
  - Notes: Settings → Models & providers shows Terminal tools: each launcher tool found or not installed, its version, sign-in state and install hint
  - Pins: `apps/interface-host/src/terminal_tests.rs::settings_terminal_tools_show_each_found_cli_version_and_an_install_hint_for_the_rest`, `electron/src/renderer/terminal-tools.test.ts::lists every launcher tool with its version, sign-in state and install hint`, `crates/interface-contracts/src/terminal.rs::terminal_tool_profiles_match_the_shared_fixture`
- **[feature]** `c6e84fa` · 2026-10-14T19:30:06-05:00 · **Threads, Code and Agent share sessions: Open in terminal, Continue as thread, Hand to teammate** (`feat/one-product-c1-20261014`)
  - Notes: A thread's menu offers Open in terminal and Hand to teammate; a Claude/Codex pane's ··· offers Continue as thread; one session never has two writers
  - Pins: `apps/interface-host/src/dispatcher_tests.rs::a_session_a_code_pane_drives_is_never_resumed_or_sent_to_from_here`, `apps/interface-host/src/threads_tests.rs::open_in_terminal_closes_the_thread_first_and_holds_it_for_one_pane`, `apps/interface-host/src/threads_tests.rs::continue_from_terminal_waits_for_the_pane_then_brings_the_session_into_threads`, `crates/interface-settings/src/threads_tests.rs::terminal_sessions_sit_under_from_terminals_until_brought_into_threads`, `electron/src/renderer/components/TerminalPrimitives.test.ts::offers Continue as thread on an agent pane's ··· menu, and says why when it cannot (review C1)`, `electron/src/renderer/components/AgentTile.test.ts::offers Open in terminal after the split, refused in words mid-turn (review C1)`, `electron/src/renderer/thread-inbox.test.ts::hands a teammate the thread's title, where it runs and how to find it, never its session`
- **[feature]** `0a8d4d9` · 2026-10-14T19:11:13-05:00 · **Signed-out providers say so and offer Sign in; Refresh finds a CLI installed since start** (`feat/diagnostics-20261014`)
  - Notes: Settings shows Signed out with Sign in (opens the CLI's login in Code); a signed-out turn offers Sign in; Refresh finds a newly installed CLI
  - Pins: `electron/src/renderer/settings.test.ts::shows a provider its own CLI says is signed out as Signed out, with Sign in where it has one`, `crates/engine-claude/src/mapper.rs::a_signed_out_claude_turn_fails_with_the_sign_in_cause`, `crates/interface-terminal/src/service_tests.rs::a_sign_in_pane_runs_the_cli_login_and_a_refreshed_catalog_serves_later_launches`, `apps/interface-host/src/dispatcher_tests.rs::an_explicit_provider_refresh_finds_the_clis_again_before_it_probes`, `apps/iface/src/doctor.rs::doctor_reports_needs_auth_for_a_signed_out_cli`, `electron/src/shared/providers-v41.test.ts::offers Sign in on a turn that failed because the CLI is signed out, and only there`
- **[fix]** `b6398d2` · 2026-10-14T18:59:51-05:00 · **One word per idea: panes, teammate conversations, Local checkout / New worktree** (`feat/one-product-20261014`)
  - Notes: Palette: Next pane; a teammate's tab: Conversation; launcher and Agent composer: Local checkout / New worktree; teammate New task: no access picker
  - Pins: `electron/src/renderer/components/AgentWorkspacePanel.test.ts::mounts the current conversation with New conversation and past conversations`, `electron/src/renderer/composer.test.ts::a teammate's task card has no access mode or session options: its own mode decides (review C2)`, `electron/src/renderer/components/TerminalLauncher.test.ts::renders only the owner's launch roster with compact provider rows`, `electron/src/renderer/workspace-ui.test.ts::keeps command-palette actions concise and explains every known disabled state`
- **[feature]** `e69ecc4` · 2026-10-14T18:48:57-05:00 · **New project shows its Projects folder, with Change… to pick another** (`feat/one-product-20261014`)
  - Notes: New project… names the folder it will create in (~/Developer/Projects by default); Change… picks another and New project keeps using it
  - Pins: `crates/interface-desktop-services/src/service_tests.rs::new_project_creates_in_the_chosen_projects_folder_and_reads_it_back`, `crates/interface-desktop-services/src/service_tests.rs::projects_folder_defaults_to_developer_projects_then_the_last_picked_parent`, `electron/src/renderer/components/CreateProjectDialog.test.ts::offers Change… beside the path only when the folder can be changed`
- **[feature]** `5bfda83` · 2026-10-14T18:35:38-05:00 · **Help menu: Reveal Logs and Copy Diagnostics; host.log; a one-time notice when a store is set aside** (`feat/diagnostics-20261014`)
  - Notes: Help → Copy Diagnostics copies a filtered report; Reveal Logs shows host.log; an unreadable threads file shows a one-time notice with Reveal
  - Pins: `apps/interface-host/src/diagnostics_tests.rs::a_set_aside_store_is_noticed_once_until_dismissed_and_reveals_its_file`, `crates/interface-review/src/diagnostics.rs::copy_diagnostics_names_the_build_and_providers_and_never_carries_tokens`, `apps/interface-host/src/host_log.rs::host_log_keeps_warnings_and_errors_but_not_info`, `electron/src/main/app-menu.test.ts::help offers Reveal Logs and Copy Diagnostics, and Settings… opens Settings`
- **[fix]** `7c2cc3b` · 2026-10-14T18:31:47-05:00 · **Pane menu drops the dead Show delegated work; delegation UI archived** (`chore/archive-delegation-ui-20261014`)
  - Notes: A terminal pane's ··· menu has no Show delegated work, and panes stop polling for workers
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::draws each pane in a plain frame, with no delegated-work item and no worker poll`
- **[record]** `4535d09` · 2026-10-14T18:27:24-05:00 · **Record the dormant-feature archive and its pending approval** (`chore/archive-dormant-20261014`)
  - Notes: Record the dormant-feature archive and its pending renderer approval
- **[internal]** `c871c4d` · 2026-10-14T18:24:30-05:00 · **Control assistant, Tailscale phone route and pane delegation (host side) archived** (`chore/archive-dormant-20261014`)
  - Notes: Retire and archive the Control assistant; Retire and archive the Tailscale phone route; Retire pane delegation from the host and crates; archive its docs; Record the pinned tests the three archived features replace
- **[feature]** `0cb4fc8` · 2026-10-14T18:02:30-05:00 · **Threads ship from the Changes panel: commit, branch, push and PR, checks, review comments, merge** (`feat/threads-ship-ui-20261014`)
  - Notes: In a project thread, the Changes panel commits the thread's files, opens a PR, fixes failing checks and merges; the card offers the next step
  - Pins: `electron/src/renderer/thread-ship.test.ts::runs one effect at a time and never retries an unknown outcome until a read settles it`, `apps/interface-host/src/threads_ship_tests.rs::a_local_thread_commits_its_own_files_opens_one_pull_request_and_merges_to_settle`, `crates/interface-settings/src/threads_tests.rs::a_pull_request_interface_opened_is_tracked_with_tracking_off`
- **[fix]** `9b74c0c` · 2026-10-14T17:53:56-05:00 · **Threads find gh in ~/.local/bin too, so PR tracking works there; stable branch suggestion** (`feat/threads-ship-20261014`)
  - Notes: With gh installed in ~/.local/bin, a worktree thread's PR chip appears in Threads
  - Pins: `crates/interface-review/src/thread_ship_tests.rs::gh_is_found_on_path_then_in_homebrew_and_user_folders`, `crates/interface-review/src/thread_ship_tests.rs::create_branch_switches_carrying_changes_and_refuses_an_existing_name`
- **[fix]** `3f509ef` · 2026-10-14T17:41:57-05:00 · **Teammate New task runs behind the agent's gate: home, MEMORY.md, folders, Ask mode** (`fix/review-quick-20261014`)
  - Notes: A teammate's New task in a project folder appears under the agent and in Runs; Ask-mode approvals go through its policy; home or ~ is refused
  - Pins: `apps/interface-host/src/agents_tests.rs::a_teammate_task_launches_behind_the_gate_with_home_folders_and_job`, `apps/interface-host/src/agents_tests.rs::a_teammate_task_is_refused_before_the_provider_and_leaves_no_run`, `crates/interface-settings/src/agents/tests.rs::a_one_off_task_is_admitted_once_as_a_starting_gated_run`, `crates/interface-contracts/src/agents.rs::v38_agent_task_start_round_trips_and_rejects_unknown_fields`, `electron/src/renderer/agent-profiles.test.ts::sends only the owner's words, folder and launch choices for Rust to gate`
- **[internal]** `37b1e78` · 2026-10-14T17:28:15-05:00 · **Threads ship commands on the wire: commit, branch, push and PR, checks, review comments, merge** (`feat/threads-ship-20261014`)
  - Notes: Threads: ship a thread's work over the wire (protocol 37)
- **[feature]** `c587210` · 2026-10-14T17:22:03-05:00 · **Notification settings; native sound respects Focus; Dock badge counts waiting panes** (`fix/review-quick-20261014`)
  - Notes: Settings › Notifications has three switches; no chime when notifications are refused; the Dock badge counts terminal panes waiting for you
  - Pins: `electron/src/renderer/notify.test.ts::uses the notification's own sound, so Focus and permission decide, and no chime (review D5)`, `electron/src/renderer/components/SettingsPage.test.ts::has a Notifications page with sound, finished and needs-input switches (review D5)`, `electron/src/renderer/dock-attention.test.ts::counts live panes waiting for input, the ones the attention tray lists`, `electron/src/shared/dock-attention.test.ts::adds waiting terminal panes to the waiting sessions`
- **[feature]** `a215090` · 2026-10-14T17:14:30-05:00 · **Palette and ··· menu reach every surface: Away, voice, iPhone, Files, Browser, themes** (`fix/review-quick-20261014`)
  - Notes: ⌘⇧P lists Away loop…, iPhone remote access, Files, Browser, Agent overview, New project and themes; ··· lists Coordination canvas, Worktrees, Context inventory
  - Pins: `electron/src/renderer/palette-commands.test.ts::reaches Away, voice, iPhone access, Files, Browser, overview, New project and themes (review C3)`, `electron/src/renderer/palette-commands.test.ts::drops Integrated terminals, which only repeated Go to Code`, `electron/src/renderer/components/AttentionTray.test.ts::reaches the Coordination canvas, Worktrees and the Context inventory from ···`
- **[fix]** `9d8dd46` · 2026-10-14T17:08:58-05:00 · **Interface keeps the Mac awake while agents, threads, teammate runs or Away loops work** (`fix/keep-awake-20261014`)
  - Notes: While an agent works, pmset -g assertions lists Interface's PreventUserIdleSystemSleep; it goes once the agent is idle or only waiting
  - Pins: `electron/src/main/keep-awake.test.ts::holds one app-suspension blocker from the first working report and releases it when idle`, `electron/src/main/keep-awake.test.ts::lets the Mac sleep when everything is only waiting for an answer`, `electron/src/main/keep-awake.test.ts::releases when the report goes away and for good on quit`, `electron/src/main/main-wiring.test.ts::keeps the Mac awake from the work-in-flight report and releases it on quit`, `electron/src/renderer/work-in-flight.test.ts::counts teammate runs in progress, once, and what only waits`, `electron/src/shared/quit-guard.test.ts::is working while anything progresses, not while everything only waits`
- **[fix]** `15657df` · 2026-10-14T17:05:39-05:00 · **Review fixes: Task details reads plainly, ACP engines off for new agents, Relaunch after a crash** (`fix/review-quick-20261014`)
  - Notes: Palette has one Task details… row with readable inventory; New agent shows Grok/Qwen disabled with the reason; a backend-stopped alert offers Relaunch
  - Pins: `electron/src/renderer/components/TaskInspector.test.ts::prints the context inventory as named groups instead of raw JSON`, `electron/src/renderer/palette-commands.test.ts::offers one Task details row instead of four rows that opened the same dialog`, `electron/src/renderer/components/AgentMenu.test.ts::never creates an agent on a provider that cannot be a teammate (review A6)`, `electron/src/main/crash-dialog.test.ts::makes Relaunch the default answer when the backend stops`, `electron/src/renderer/settings.test.ts::reads a store that still holds the retired editor settings and drops them`
- **[feature]** `d266642` · 2026-10-14T17:03:46-05:00 · **Browser tab in the Agents column; Browser panes match its look** (`feat/browser-sidebar-look-20261014`)
  - Notes: The Agents column has a Browser tab with a working browser, and Browser panes use the column's rounded search field and page card
  - Pins: `electron/src/renderer/agent-overview.test.ts::offers a Browser tab that shows its body in place of the agents`
- **[record]** `ff91056` · 2026-10-14T17:01:53-05:00 · **TODO: review A1-A5 landing and what was not seen on screen** (`fix/review-a1-a5-20261014`)
  - Notes: TODO: record the review A1-A5 landing and what was not seen on screen
- **[fix]** `a4ee5d7` · 2026-10-14T17:01:13-05:00 · **Review fixes: Away follows focus, Quit asks first, Start again, provider alert, ready models first** (`fix/review-a1-a5-20261014`)
  - Notes: Away opens on the focused thread's project; Cmd+Q asks while agents work; crashed panes offer Start again; provider alert opens Models & providers
  - Pins: `electron/src/renderer/away-view.test.ts::works on the project in front: the focused thread on Threads, the terminal on Code`, `electron/src/renderer/away-view.test.ts::opens the running loops instead of starting another`, `electron/src/renderer/away-view.test.ts::asks before the first start in a project, then starts at once`, `electron/src/shared/quit-guard.test.ts::asks how many agents are still working and names what stops`, `electron/src/main/main-wiring.test.ts::asks before a quit would end working agents`, `electron/src/renderer/terminal-shell-handoff.test.ts::offers a restart on every agent end the hand-off leaves alone`, `electron/src/renderer/components/TerminalPrimitives.test.ts::offers Start again on an ended agent pane, above xterm's own layers`, `electron/src/renderer/components/SettingsPage.test.ts::opens on the section it is asked for`, `electron/src/renderer/settings.test.ts::says what to do about a provider probe failure, keeping the CLI's own words`, `electron/src/renderer/composer-model.test.ts::lists unavailable providers after the ready ones, each in registration order`, `electron/src/renderer/composer-model.test.ts::opens a fresh card on a ready provider's model, never on Choose model`
- **[fix]** `0576c7e` · 2026-10-14T16:57:01-05:00 · **Threads turns keep their end and own files; durations survive restarts; shared Review is scoped** (`feat/threads-ship-20261014`)
  - Notes: After a Dev restart earlier Threads turns still read 'Worked for …'; with two threads in one Local checkout a turn's Review shows only that turn's files
  - Pins: `apps/interface-host/src/threads_tests.rs::a_shared_checkout_turn_keeps_its_end_and_files_and_its_review_diff_covers_only_them`, `crates/interface-settings/src/threads_tests.rs::a_v1_store_is_carried_over_once_and_left_unchanged`, `electron/src/renderer/thread-transcript.test.ts::times an earlier turn from its send to its own end, after the next send and a restart`

### v0.3.157 — 2026-10-12T14:24:38-05:00 (`384af28`)

- **[feature]** `20f5e05` · 2026-10-12T13:05:21-05:00 · **Muse Code is a terminal option** (`muse-code-profile`)
  - Notes: New-terminal launcher and + menu list Muse Code with an M mark; choosing it runs muse in the picked folder (marked unavailable until Muse Code is installed)
  - Pins: `crates/interface-terminal/src/launch_tests.rs::muse_launches_directly_and_refuses_resume_and_pass_off_bindings`, `electron/src/renderer/components/TerminalLauncher.test.ts::offers Muse Code with its own mark beside the other agents`, `electron/src/renderer/components/TerminalPrimitives.test.ts::leaves Muse Code out of Pass off because its TUI takes no initial prompt`
- **[internal]** `9bda789` · 2026-10-09T13:59:02-05:00 · **Threads: measure a 300-turn transcript (no windowing needed)** (`test/threads-long-transcript-20261009`)
  - Notes: Threads: measure a 300-turn transcript in the threads DOM lane
- **[feature]** `b993eb7` · 2026-10-09T13:55:09-05:00 · **Threads: Retry a failed turn** (`feat/threads-retry-turn-20261009`)
  - Notes: When a thread's latest turn fails, Retry sits under it; one click sends the same message again in that thread (text only), once
  - Pins: `electron/src/renderer/thread-turns.test.ts::offers Retry on the newest turn when Rust recorded it failed`, `electron/src/renderer/thread-turns.test.ts::never offers an older failure once a later turn was sent`
- **[feature]** `90d3da6` · 2026-10-09T13:47:36-05:00 · **Threads: Changes panel can show a diff side by side (Stacked / Split)** (`feat/threads-split-diff-land-20261009`)
  - Notes: Open Changes in a wide window and press Split: old file left, new file right, line comments still land in the prompt; Stacked stays the default
  - Pins: `electron/src/renderer/components/ThreadDiffPanel.test.ts::pairs three deletions with one addition row by row, then fills the right`, `electron/src/renderer/components/ThreadDiffPanel.test.ts::draws Split only when it is chosen and the panel is wide enough`
- **[feature]** `b8ebda0` · 2026-10-09T13:45:37-05:00 · **Threads: Undo turn in the Changes panel for the turn it shows** (`feat/threads-panel-undo-20261009`)
  - Notes: In a local thread, open Changes on one turn: Undo turn in the panel header restores files to before that message, after the same confirm
  - Pins: `electron/src/renderer/thread-turns.test.ts::finds the restore point on the row the pane matched the turn to`

### v0.3.156 — 2026-10-07T16:30:09-05:00 (`e37c6d0`)

- **[fix]** `64ee194` · 2026-10-07T15:12:09-05:00 · **Threads keep their permission mode (Auto, Plan…) after Interface restarts** (`fix/resume-keeps-access-20261007`)
  - Notes: Start a thread in Auto, quit and reopen Interface, then send a follow-up: it stays in Auto and runs tools without asking.
  - Pins: `apps/interface-host/src/dispatcher_tests.rs::a_session_resumes_after_a_restart_in_the_mode_its_owner_chose`
- **[look]** `482c3e4` · 2026-10-07T15:00:49-05:00 · **Threads live turn: calmer — no composer arc, a still head line, steady step words** (`feat/calm-live-turn-20261007`)
  - Notes: Run a Threads turn: only the Wave and the step's ring move; the composer edge and head line stay still, step words don't shimmer
  - Pins: `electron/src/renderer/components/ThreadComposer.test.ts::draws nothing that moves on the box's edge (owner: less motion)`, `electron/src/renderer/thread-view.test.ts::says the newest running step under the prompt, verb then project-relative target`
- **[record]** `3275f58` · 2026-10-07T11:09:08-05:00 · **TODO: the Threads live turn shipped in 0.3.155** (`record/todo-0155`)

### v0.3.155 — 2026-10-07T11:04:52-05:00 (`a644b6c`)

- **[record]** `5c219bf` · 2026-10-07T10:59:49-05:00 · **TODO: the Threads live turn is on Dev** (`design/threads-running-20261007`)
- **[look]** `21c38df` · 2026-10-07T10:59:18-05:00 · **Threads live turn: the run draws under its prompt, approvals dock, the inbox shows live steps** (`design/threads-running-20261007`)
  - Notes: Start a Threads turn: steps trail under your prompt with one clock, fold into Worked for…, and an approval docks above the composer
  - Pins: `electron/src/renderer/thread-view.test.ts::says the newest running step under the prompt, verb then project-relative target`, `electron/src/renderer/thread-view.test.ts::docks a waiting approval out of the transcript: the step waits, no card`, `electron/src/renderer/thread-view.test.ts::keeps a finished turn's time once the next turn is sent`, `electron/src/renderer/thread-view.test.ts::clips the transcript's region, so nothing inside it can scroll the tile`, `electron/src/renderer/components/ThreadInbox.test.ts::says on line two what a live thread is doing, asking or failed on, in the project line's place`
- **[fix]** `3149e14` · 2026-10-07T10:56:50-05:00 · **Two threads in one checkout: each turn counts only its own files** (`feat/liveturn-s5-difffix`)
  - Notes: Run two Local threads in one project at once: each turn's changes card counts only that thread's own files
  - Pins: `apps/interface-host/src/threads_tests.rs::a_turn_in_a_shared_checkout_counts_only_the_files_its_own_edits_named`, `crates/interface-review/src/thread_git_tests.rs::scoped_turn_numstat_counts_only_named_files_inside_the_repository`
- **[look]** `b6e0144` · 2026-10-07T09:38:20-05:00 · **Agent replies stream in steadily instead of arriving in chunks** (`feat/threads-stream-20261007`)
  - Notes: In a Threads pane an Opus reply flows in without pausing between bursts, and its last lines flow in instead of popping on when the turn ends
  - Pins: `electron/src/renderer/conversation.test.ts::keeps an Opus-style burst flowing until the next one lands`, `electron/src/renderer/conversation.test.ts::finishes a completed message within 400 ms instead of snapping`
- **[fix]** `60d12b1` · 2026-10-07T09:37:15-05:00 · **New threads open at once in iCloud projects: checkpoints stop reading evicted git objects** (`fix/thread-launch-slow-20261007`)
  - Notes: Start a thread in Kestrel AI (on the Desktop): it opens in under a second, not ~10 s, and its first turn has a checkpoint for Changes.
  - Pins: `crates/interface-review/src/thread_git_tests.rs::checkpoint_capture_never_reads_head_trees_so_an_evicted_object_cannot_stall_it`

### v0.3.154 — 2026-10-07T09:09:17-05:00 (`8ee7519`)

- **[fix]** `7fb2f0f` · 2026-10-07T07:46:16-05:00 · **Codex Untrusted mode: correct its description (the app server still accepts it)** (`fix/codex-untrusted-20261007`)
  - Notes: The Codex Untrusted row no longer warns that sessions may refuse to start
  - Pins: `electron/src/renderer/access-modes.test.ts::names Codex's, Grok's and the ACP providers' modes in their own words`
- **[look]** `86dba02` · 2026-10-06T22:44:36-05:00 · **Access modes use each provider's own names (Claude Manual, Codex Ask for approval, Grok)** (`feat/provider-mode-names-20261006`)
  - Notes: The access picker and status line name each provider's modes as its CLI does, e.g. Claude Code's Manual / Accept edits / Plan / Auto
  - Pins: `electron/src/renderer/access-modes.test.ts::names Claude Code's modes as Claude Code does, picker and status line`, `electron/src/renderer/access-modes.test.ts::names Codex's, Grok's and the ACP providers' modes in their own words`

### v0.3.153 — 2026-10-06T22:19:01-05:00 (`fae1062`)

- **[record]** `b449c3e` · 2026-10-06T22:18:43-05:00 · **Threads: record the color round and the inbox row pin it superseded** (`feat/threads-ui-redesign-20261006`)
  - Notes: TODO: the Threads color round is built; the inbox row pin moved with it
- **[look]** `abae39a` · 2026-10-06T21:57:19-05:00 · **Threads hued glow: project hues and tiles, status pills, hued composer, syntax-colored diffs** (`feat/threads-ui-redesign-20261006`)
  - Notes: Threads: each project has a color; the open thread's bubbles, composer and Send take it; the inbox shows project tiles and status pills
  - Pins: `electron/src/renderer/project-hue.test.ts::is stable for a folder, whatever its trailing slash or case`, `electron/src/renderer/project-hue.test.ts::splits five squares by added and removed lines, showing each side that changed`
- **[look]** `8117db1` · 2026-10-06T21:04:13-05:00 · **Threads redesign: two-line inbox, folded turns, chip composer, centred new thread, diff comments** (`feat/threads-ui-redesign-20261006`)
  - Notes: Threads: inbox rows show status or age, turns fold into Worked for…, Changes panel comments land in the draft
  - Pins: `electron/src/renderer/components/ThreadInbox.test.ts::draws Pinned and Active as two lines: title and one slot, then pip, project, branch and the PR`, `electron/src/renderer/thread-transcript.test.ts::counts the folded work by kind: distinct files read, files edited, commands run`, `electron/src/renderer/components/ThreadDiffPanel.test.ts::follows the owner's words, one blank line apart, and never replaces them`, `electron/src/renderer/components/AgentTile.test.ts::reads as a breadcrumb: the project, then the title`
- **[record]** `de9130d` · 2026-10-06T17:03:56-05:00 · **TODO: Threads shipped in 0.3.152** (`record/threads-shipped-0.3.152`)

### v0.3.152 — 2026-10-06T16:59:50-05:00 (`7be0457`)

- **[feature]** `b745079` · 2026-10-06T13:38:13-05:00 · **Threads: Chat becomes a T3-style inbox with project threads, worktrees, turn diffs and fork/edit** (`feat/threads-mode-20261006`)
  - Notes: Threads tab: New thread picks a project (Local or New worktree); inbox sidebar; Cmd+D shows a turn's diff; Fork/Edit from here make new threads
  - Pins: `crates/interface-review/src/thread_git_tests.rs::checkpoint_capture_never_changes_git_status_or_the_index`, `crates/interface-review/src/thread_git_tests.rs::restore_returns_to_a_checkpoint_and_keeps_a_recovery_ref`, `crates/interface-review/src/thread_git_tests.rs::ignored_files_keep_a_worktree_on_release_and_discard`, `apps/interface-host/src/threads_tests.rs::metadata_commands_answer_the_view_and_push_after_each_committed_change`, `apps/interface-host/src/threads_tests.rs::send_hook_records_a_turn_with_the_prompt_digest_and_its_start_checkpoint`, `electron/src/renderer/thread-inbox.test.ts::draws Rust's four sections from the shared v34 view, in inbox order`, `electron/src/renderer/thread-card.test.ts::offers the chat engines only with No project, every engine with a project`, `electron/src/renderer/thread-view.test.ts::puts the change summary and Edit from here on matched prompts only`
- **[record]** `a52b928` · 2026-10-06T09:46:25-05:00 · **To-do: performance review rounds 1 and 2 landed; what is left on purpose** (`docs/perf-review-todo-20261006`)
- **[fix]** `7344a8d` · 2026-10-06T09:33:40-05:00 · **Claude panes turn their sidebar pip amber while a permission prompt or question waits** (`feat/claude-waiting-signal-20261006`)
  - Notes: A Claude pane asking for permission or asking you a question shows an amber pip and ⚠ in the sidebar until you answer
  - Pins: `apps/interface-host/src/claude_terminal_hook_tests.rs::a_permission_dialog_marks_the_pane_waiting_without_deciding_it`, `electron/src/renderer/terminal-activity.test.ts::needs you from a permission or question frame until it is answered`
- **[record]** `9dae1f0` · 2026-10-06T09:24:18-05:00 · **To-do: Claude reviewer read-only repository tools done** (`record/claude-review-read-tools-20261006`)
  - Notes: To-do: Claude reviewer read-only repository tools done; friction-fixes entry moved to done
- **[fix]** `d10e659` · 2026-10-06T09:22:58-05:00 · **Version checks no longer time out on finished CLIs; retired settings channel; leaner host code** (`fixall/host-20261006`)
  - Notes: A Claude, Codex or OpenCode version check returns when the CLI exits, even if another process holds its output pipe
  - Pins: `crates/interface-engine/src/executable.rs::a_version_probe_ends_when_the_cli_exits_though_its_pipe_stays_open`, `crates/interface-engine/src/child_output.rs::output_ends_with_the_child_though_an_outside_process_holds_the_pipe`
- **[record]** `2258e44` · 2026-10-06T09:19:45-05:00 · **Shorter root AGENTS.md: hands-free and release traps move to their own docs** (`docs/agents-md-trim-20261006`)
- **[speed]** `4fb7973` · 2026-10-06T09:18:45-05:00 · **Hidden terminals skip screen-reader work; two DOM-lane flakes fixed; App.tsx slimmer** (`fixall/renderer-20261006`)
  - Notes: Renderer DOM lanes run 4 at a time beside the Rust tests; terminals out of view for a minute stop building announcements
- **[feature]** `b999221` · 2026-10-06T08:55:25-05:00 · **iface theme convert: turn iTerm2, Ghostty, Alacritty, Base16 or VS Code schemes into theme files** (`feat/theme-convert-20261006`)
  - Notes: Run iface theme convert <scheme file> > ~/.config/interface/themes/Name.css and pick the theme in Settings
  - Pins: `crates/interface-settings/src/theme_import_tests.rs::converted_css_reads_back_as_a_theme_file`, `apps/iface/src/theme.rs::theme_convert_prints_css_and_suggests_the_file_name`
- **[speed]** `f302f03` · 2026-10-06T08:48:04-05:00 · **Voice assistant sees the 50 most recent sessions, not all of them** (`fix/voice-session-cap-20261006`)
  - Notes: Ask the voice assistant about your sessions: it answers from the 50 most recent and can still find an older one by name
  - Pins: `apps/interface-host/src/voice/tools.rs::the_assistant_sees_the_fifty_most_recent_sessions_and_the_full_count`, `apps/interface-host/src/voice/tools.rs::a_query_finds_older_sessions_by_title_folder_or_engine`
- **[fix]** `eaa08aa` · 2026-10-06T08:45:33-05:00 · **Shell handoff stays in its own workspace after a switch** (`fix/handoff-own-workspace-20261006`)
  - Notes: Switch workspace while an exited agent's shell is starting: the shell opens in the agent's own workspace, not the one you switched to
  - Pins: `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalHandoffSwitchFixture`
- **[internal]** `d901be5` · 2026-10-06T08:41:28-05:00 · **Gate stops rerunning two load-flaky tests and keeps receipts across an Xcode or node change** (`fixall/iface-20261006`)
  - Notes: The gate no longer reruns two tests that only failed under load; Gate receipts survive an Xcode or node change on the other side; Control-desk child test keeps reading the tool-less host until it exits
- **[fix]** `737dd2b` · 2026-10-06T08:35:15-05:00 · **Model-facing prompts and tool descriptions match what the tools do** (`prompt-audit-20261006`)
  - Notes: Claude panes show the cheap-agents instructions once, and the voice assistant no longer treats every session as text rehearsal
  - Pins: `apps/interface-host/src/pane_mcp.rs::pane_tools_name_only_ids_pane_sessions_can_obtain`, `crates/interface-collaboration/src/workers/tools.rs::worker_tools_describe_the_contracts_their_handlers_enforce`, `crates/engine-claude/src/adapter_tests.rs::voice_prompt_does_not_key_speech_on_the_constant_audio_flag`
- **[fix]** `99273fe` · 2026-10-06T08:32:03-05:00 · **Quieter host while idle and streaming; new panes no longer fail when a job grabs the terminal first** (`fixall/host-20261006`)
  - Notes: A pane whose shell starts a job at once opens instead of failing; the sidebar re-sorts by activity at most every 5 s
  - Pins: `crates/interface-terminal/src/embedded.rs::spawn_identity_accepts_a_job_that_already_took_the_foreground`, `crates/interface-terminal/src/foreground_profile.rs::a_job_without_an_agent_backs_off_to_thirty_seconds_after_its_first_minute`, `crates/interface-remote/src/access.rs::output_is_kept_only_while_phone_access_is_on_and_always_counted`
- **[fix]** `ed1dabb` · 2026-10-06T08:28:59-05:00 · **Review panes stay open in Chat; Open in external terminal works again** (`fixall/renderer-20261006`)
  - Notes: Builder/reviewer panes open in Chat and stay across refreshes; Open in external terminal is enabled on Code's terminals
  - Pins: `electron/src/renderer/conversation-layouts.test.ts::keeps an adversarial-review pair open across registry pushes`, `electron/src/renderer/conversation-projection.test.ts::keeps a sent message's echo under the row it followed when compaction drops the head`, `electron/src/renderer/draft-store.test.ts::publishes a typed word once, its clearing once, and every send's republish`
- **[fix]** `53d02e3` · 2026-10-06T08:17:26-05:00 · **New panes split the pane they open from** (`fix/split-pane-in-place`)
  - Notes: A pane's + → Terminal halves that pane (side by side if wide, stacked if tall); no other pane moves and the new pane fades in once
  - Pins: `electron/src/renderer/terminal-workspace.test.ts::halves only the pane it was opened from; no other pane moves`, `electron/src/renderer/terminal-workspace.test.ts::gives the same layout when React runs the updater again`, `electron/src/renderer/fixtures/terminal-pane-motion-fixture.tsx::terminalPaneMotionFixture`
- **[fix]** `d69cfcf` · 2026-10-06T08:12:35-05:00 · **iface agent list fits any registry; agent stop never waits on another agent's start** (`fixall/iface-20261006`)
  - Notes: iface agent list answers with thousands of stored sessions (live first, --live for only live); iface agent stop returns while another agent is still starting
  - Pins: `apps/iface/src/agent_server.rs::a_long_list_keeps_live_and_named_sessions_and_fits_one_reply`, `apps/iface/src/agent_server.rs::a_stop_does_not_wait_for_another_sessions_start_or_resume`, `apps/iface/src/agent_server.rs::the_mailbox_prunes_settled_messages_but_keeps_unknown_ones_and_their_parents`, `apps/iface/src/tui_tests.rs::a_huge_diff_keeps_a_capped_side_and_details_say_so`
- **[internal]** `fc74063` · 2026-10-06T01:27:12-05:00 · **Codex adapter and session management split into smaller files; review to-do entry** (`perf/split-20261005`)
  - Notes: Codex adapter and session management code split into smaller files; To-do: October 5 performance review — what landed and what waits on the owner; Gate reruns two load-flaky tests once instead of failing
- **[speed]** `3564679` · 2026-10-06T01:02:25-05:00 · **Faster gate: Rust tests run four at a time beside Clippy; steadier timing tests** (`perf/gate-20261005`)
  - Notes: cargo xtask check prints a per-executable time table; the Rust half takes about 55 s instead of about 105 s; an Electron-only gate runs 5 renderer lanes
- **[internal]** `fc91a9f` · 2026-10-06T00:22:00-05:00 · **Disabled pane delegation no longer built by default; simpler orchestration and capture-hook code** (`perf/rust-20261005`)
- **[speed]** `dc64182` · 2026-10-06T00:08:31-05:00 · **One saved-layout parse for terminals; App.tsx and conversation modules untangled** (`perf/renderer-20261005`)
  - Notes: Splitter drags and roster reads no longer re-parse every saved workspace layout
- **[speed]** `a2c3697` · 2026-10-05T23:56:53-05:00 · **Less background work: no usage reads while hidden, lighter streaming, one save per slider drag** (`perf/renderer-20261005`)
  - Notes: With the window hidden no usage CLI is spawned; dragging a translucency slider writes appearance once; long chats stream with less lag
- **[speed]** `36c392f` · 2026-10-05T23:54:33-05:00 · **Fewer drive flushes and process spawns; faster Quick Open and Claude listing at launch** (`perf/rust-20261005`)
  - Notes: Quick Open opens faster on large projects; each store write and decision-log append waits on at most one full drive flush
- **[internal]** `dacee77` · 2026-10-05T23:39:23-05:00 · **Every agent's Dev refresh stays hidden, even from older branches** (`fix/quiet-dev-refresh-20261005`)
- **[speed]** `50885bd` · 2026-10-05T23:34:56-05:00 · **Terminals give back GPU memory when out of view, and the work area drops a hidden layer** (`perf/renderer-20261005`)
  - Notes: Leave a workspace or tab for a minute, or hide the window for two: the GPU helper's footprint drops; panes come back on WebGL with crisp text
- **[speed]** `19703e7` · 2026-10-05T23:31:24-05:00 · **Lighter registry pushes; pane output no longer stalls behind sleep probes or agent saves** (`perf/rust-20261005`)
  - Notes: Switching away from a workspace no longer pauses pane output; host CPU stays lower while agents stream
- **[internal]** `e34b28b` · 2026-10-05T23:28:01-05:00 · **Dev refresh no longer takes the screen** (`fix/quiet-dev-refresh-20261005`)
- **[fix]** `cacc353` · 2026-10-05T22:53:18-05:00 · **Codex chats keep working after a failed turn; stale socket and temp files get cleaned up** (`perf/rust-20261005`)
  - Notes: After a failed Codex turn (e.g. a usage limit) the composer still sends; after a Dev restart the collaboration sockets folder holds only live sockets
  - Pins: `crates/interface-engine/src/lifecycle.rs::notice_does_not_move_status_and_a_failed_turn_keeps_the_handle_sendable`, `crates/engine-codex/src/adapter_tests.rs::a_failed_turn_leaves_the_session_sendable`, `crates/interface-storage/src/atomic.rs::a_write_that_fails_before_the_rename_removes_its_temporary`, `crates/interface-collaboration/tests/suite/broker_runtime.rs::start_removes_dead_sockets_and_leaves_live_ones_and_other_files`
- **[fix]** `56d4380` · 2026-10-05T22:50:45-05:00 · **Chat rows stay put through compaction, snake_case renders intact, quit never strands the host** (`perf/renderer-20261005`)
  - Notes: max_tokens keeps its underscores in chat; the Dev log stops filling with stale collaboration-draft revisions; quitting after a window crash still stops the host
  - Pins: `electron/src/renderer/draft-store.test.ts::stays above the host's mark after a reload forgets the counter`, `electron/src/renderer/markdown.test.ts::leaves intraword underscores and spaced asterisks as the characters they are`, `electron/src/main/main-wiring.test.ts::stops the host even when an earlier teardown step throws`, `electron/src/renderer/conversation-projection.test.ts::keeps every surviving row's key and item when compaction drops the log's head`
- **[internal]** `840c7b0` · 2026-10-05T22:44:02-05:00 · **Phone relay on Cloudflare Workers (off-Wi-Fi phone access)** (`feat/phone-relay-worker-20261005`)
  - Notes: Phone relay on Cloudflare Workers; TODO: phone relay setup steps for the owner
- **[internal]** `a4c17eb` · 2026-10-05T22:39:05-05:00 · **Keep the main checkout on dev; looser release and look approvals** (`docs/agent-friction-approved-20261005`)
- **[look]** `972a887` · 2026-10-05T22:20:47-05:00 · **Narrower agent overview column (360–560px)** (`fix/narrow-right-sidebar-20261005`)
  - Notes: The right-hand agent overview is narrower: 560px on a 2560pt window, 360px at 1440px
  - Pins: `electron/src/renderer/agent-overview.test.ts::takes 360px at a 1440px window and stops at 560px on a large display`
- **[record]** `1bd070d` · 2026-10-05T22:17:52-05:00 · **To-do: agent workflow friction fixes awaiting the owner** (`docs/agent-friction-20261005`)
- **[record]** `dd51cc0` · 2026-10-05T22:13:21-05:00 · **Lighter agent start-up rules and a shorter to-do list** (`docs/agent-friction-20261005`)
- **[feature]** `5afb4a4` · 2026-10-02T12:00:57-05:00 · **Theme files: your own .css themes in ~/.config/interface/themes** (`feat/custom-themes-20261002`)
  - Notes: A .css file in ~/.config/interface/themes shows under Yours in Settings; picking it repaints the app, and saving the file repaints it live
  - Pins: `crates/interface-desktop-services/src/service_tests.rs::theme_files_v5_list_and_paint`, `electron/src/renderer/components/SettingsPage.test.ts::lists theme files under Yours and pairs Follow macOS with any theme`, `electron/src/renderer/theme-files.test.ts::sit inside the interface-palette layer, so a file's plain :root rule wins`, `apps/interface-host/src/theme_watch.rs::creates_the_folder_and_pushes_once_per_change`

### v0.3.151 — 2026-10-04T08:42:21-05:00 (`746137e`)

- **[fix]** `457a17f` · 2026-10-03T21:49:00-05:00 · **New projects land in ~/Developer/Projects** (`fix/new-project-developer-20261003`)
  - Notes: Workspaces + → New project… shows ~/Developer/Projects/<name> and creates the folder there
  - Pins: `crates/interface-desktop-services/src/service_tests.rs::create_project_creates_and_grants_the_exact_named_folder_from_the_shared_fixture`
- **Left on Dev:** 7e71b2e Theme files: your own .css themes in ~/.config/interface/themes

### v0.3.150 — 2026-10-02T11:15:58-05:00 (`cf6dfa3`)

- **[look]** `b211604` · 2026-10-02T11:03:10-05:00 · **Roomier look: 32px heads and rows, 12px gutters, airier terminal lines** (`feat/roomier-d-20261002`)
  - Notes: Pane heads and sidebar rows are 32px, gutters 12px, terminal lines visibly airier, and the agent overview cards are larger
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps adjacent block-art and box-drawing rows visually joined`, `electron/src/renderer/theme-accessibility.test.ts::pins visible focus, minimum shared hitboxes, and container-aware pane layout`
- **[fix]** `781ecf3` · 2026-10-02T10:43:49-05:00 · **Sidebar pips take their agent's colour: Codex green, Claude rust** (`fix/provider-pips-20261002`)
  - Notes: A workspace with a Claude and a Codex pane shows a rust pip and a green pip; a working shell's pip is grey
  - Pins: `electron/src/renderer/viewmodels.test.ts::names each agent pane's provider so Codex and Claude pips differ`, `electron/src/renderer/components/Sidebar.test.ts::keeps its visual rules component-scoped and token driven`

### v0.3.149 — 2026-10-01T21:34:18-05:00 (`59f9cb9`)

- **[look]** `8c511c2` · 2026-10-01T21:22:16-05:00 · **Polished workspace launcher: setup state, joined count, folder name, ⌘↵ launch** (`feat/launcher-refine-20261001`)
  - Notes: New workspace shows no intro line, one joined 1–8 control, the folder's name before ~/path, and ⌘↵ launches
  - Pins: `electron/src/renderer/components/TerminalLauncher.test.ts::shows a home folder the way a terminal prints it, keeping the absolute path`, `electron/src/renderer/components/TerminalLauncher.test.ts::counts the shared folder note and launches on ⌘↵ only`

### v0.3.148 — 2026-10-01T20:26:36-05:00 (`05fe480`)

- **[look]** `1e08ada` · 2026-10-01T20:20:40-05:00 · **Colourful update button: a blue-violet tile with its words** (`feat/update-button-d4-20261001`)
  - Notes: When an update exists, the rail shows a blue-violet tile and gradient words; ready adds a popping dot and drifting colour
  - Pins: `electron/src/renderer/components/UpdateButton.test.ts::marks ready so the dot pops and the words drift`, `electron/src/renderer/components/UpdateButton.test.ts::turns the tile into a ring that fills with the download`

### v0.3.147 — 2026-10-01T19:57:41-05:00 (`f76e6cf`)

- **[fix]** `c492329` · 2026-10-01T17:27:57-05:00 · **Sidebar pips: one per open pane, urgent ones first** (`fix/sidebar-pips-20261001`)
  - Notes: A workspace with 3 open panes shows 3 pips; a finished or waiting pane's pip shows even past 8
  - Pins: `electron/src/renderer/viewmodels.test.ts::drops exited terminals and caps the row with a count`, `electron/src/renderer/components/Sidebar.test.ts::counts open tabs without dropping saved members from workspace identity`
- **[fix]** `d99a87f` · 2026-10-01T17:21:19-05:00 · **A finished agent's pane gets its shell back even if its workspace was parked (Codex update)** (`fix/codex-update-restart`)
  - Notes: Update Codex in a pane and switch workspace meanwhile: coming back, the pane is a working shell, not a dead 'Update ran successfully' screen.
  - Pins: `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalExitHandoffFixture`
- **[record]** `4f6188f` · 2026-10-01T12:52:33-05:00 · **TODO: hidden production terminals left for the owner** (`fix/orphan-terminals-20261001`)
- **[fix]** `d8dc83f` · 2026-10-01T12:50:42-05:00 · **Closing a tab ends its terminal; hidden terminals are cleaned up automatically** (`fix/orphan-terminals-20261001`)
  - Notes: Closing a tab or pane ends its process, and a relaunch no longer respawns closed tabs unseen
  - Pins: `electron/src/renderer/terminal-workspaces.test.ts::does not respawn an agent whose tab an earlier run closed`, `electron/src/renderer/terminal-hidden-sweep.test.ts::ends a quiet hidden terminal only once the whole grace has passed`
- **[record]** `efabd9d` · 2026-10-01T12:10:10-05:00 · **TODO: 0.3.146 shipped the dense look and the September 30 fixes** (`docs/shipped-03146-20261001`)

### v0.3.146 — 2026-10-01T12:04:54-05:00 (`1e339b7`)

- **[record]** `dd20a13` · 2026-10-01T12:02:40-05:00 · **TODO: the owner declined a context figure in pane heads** (`docs/no-context-20261001`)
- **[look]** `45bd9d9` · 2026-10-01T11:20:12-05:00 · **Closer to the reference: overview tabs and toggle, card pills, 8px gutters** (`feat/dense-closer-20261001`)
  - Notes: Dark theme: Agents/Tasks tabs and a status pill on each overview card; overview toggle at the rail's right edge; 8px gaps
  - Pins: `electron/src/renderer/agent-overview.test.ts::shows a live card's last lines, its clock, and how to open a waiting pane`, `electron/src/renderer/workspace-ui.test.ts::uses integrated macOS chrome with a centered surface switch and Code destinations`
- **[look]** `117dc3f` · 2026-10-01T11:03:00-05:00 · **Dense look: flush tiles, live agent cards and sidebar pips** (`feat/dense-look-20261001`)
  - Notes: Dark theme: flush black panes with thin heads, one status pip per pane in the sidebar, and live cards showing each agent's last lines in the overview
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::draws a rounded selection box around the focused terminal pane`, `electron/src/renderer/agent-overview.test.ts::shows a live card's last lines, its clock, and how to open a waiting pane`, `electron/src/renderer/viewmodels.test.ts::draws one light per terminal, in order, with the pane's own precedence`
- **[record]** `9726190` · 2026-10-01T08:10:30-05:00 · **September 30 review handoff: release ready for the owner** (`docs/review-handoff-20260930`)
  - Notes: TODO: the September 30 review's release is ready for the owner to promote
- **[speed]** `3c83149` · 2026-10-01T08:08:33-05:00 · **Quieter terminal ticker, phone roster reads and TUI draw loop; MCP sockets re-verified** (`perf/wave2-20260930`)
  - Notes: Idle panes no longer start helper threads every second, and phone workspace lists stay responsive during saves
- **[speed]** `46d3580` · 2026-10-01T08:02:54-05:00 · **Less work per token, pane, output chunk and window focus; quicker quits and gate** (`perf/fixes-20260930`)
  - Notes: Busy agent panes stay smooth, quitting with many panes is quicker, and a window focus no longer re-renders the app
- **[fix]** `b1bc3d9` · 2026-10-01T07:37:38-05:00 · **Long Claude answers end their turn; registries keep saving; menus stay open while chats stream** (`fix/edge-cases-20260930`)
  - Notes: A Claude pane gets its done dot after a long answer, and the model picker stays open while a reply streams
  - Pins: `apps/interface-host/src/claude_terminal_hook_tests.rs::long_turn_boundary_payloads_still_report_activity`, `crates/interface-registry/src/session_tests.rs::a_registry_of_ten_thousand_sessions_still_saves_and_reloads`, `crates/engine-codex/src/adapter_tests.rs::a_refused_first_turn_keeps_the_owed_collaboration_source`, `electron/src/main/terminal-data-transport.test.ts::keeps a late input frame a no-op after host EOF and the pane's release`, `electron/src/renderer/terminal-activity.test.ts::ignores OSC 9 from panes that are not Codex`, `electron/src/renderer/components/scroll-anchor.test.ts::counts only scrolls of the anchor's ancestors and the viewport`
- **[fix]** `d702526` · 2026-09-30T00:25:57-05:00 · **Recover stopped orphan Claude panes** (`fix/claude-orphan-recovery-20260929`)
  - Notes: Saved Claude panes reopen their original conversations after stopped processes outlive a host.
  - Pins: `apps/interface-host/src/claude_terminal_session_tests.rs::startup_recovers_real_stopped_orphans_and_preserves_the_session_across_two_cold_starts`, `apps/interface-host/src/shutdown.rs::terminal_cleanup_finishes_before_a_slow_service_drain_starts`, `crates/interface-terminal/src/embedded/orphan.rs::orphan_recovery_requires_exact_stopped_owned_group_and_absent_parent`
- **[record]** `96a6a58` · 2026-09-28T20:10:05-05:00 · **Codex-update dead pane investigation notes** (`fix/codex-update-pane`)
  - Notes: Record the Codex-update dead pane investigation
- **[feature]** `e9353c4` · 2026-09-28T18:24:56-05:00 · **Collapse all folders from the sidebar header** (`feat/collapse-all-folders-20260928`)
  - Notes: A button after Filter in the Workspaces header folds every folder; clicked again it opens them all
  - Pins: `electron/src/renderer/components/Sidebar.test.ts::offers Collapse all while any project is open and Expand all once every one is shut`, `electron/src/renderer/components/Sidebar.test.ts::collapses the listed projects and reopens only those`

### v0.3.144 — 2026-09-28T22:33:01-05:00 (`92404ae`)

- **[look]** `e1ea1a9` · 2026-09-28T22:17:31-05:00 · **Ten themes, their own terminal colours and the UI review polish** (`feat/ui-review-20260928`)
  - Notes: Settings shows ten theme swatches; light-theme terminals print readable colours; a pane waiting on you shows an amber strip
  - Pins: `electron/src/renderer/terminal-palettes.test.ts::keeps every colour legible on its own screen`, `electron/src/renderer/components/SettingsPage.test.ts::picks themes from swatches and switches, never a native select`, `electron/src/shared/appearance.test.ts::maps the retired palettes to the themes that replaced them`
- **Left on Dev:** d6fd59f Collapse all folders from the sidebar header

### v0.3.143 — 2026-09-28T20:29:26-05:00 (`e24bcd7`)

- **[feature]** `d6c7b44` · 2026-09-28T19:54:32-05:00 · **Browser pane opens Google and searches from the address bar** (`feat/browser-home-search-20260928`)
  - Notes: Pane + → Browser opens Google with the address bar focused; typing words searches Google; a running dev server shows as a localhost button
  - Pins: `electron/src/renderer/browser-pane.test.ts::searches Google for anything that is not an address`, `electron/src/renderer/browser-pane.test.ts::opens a new tab on Google`
- **Left on Dev:** d6fd59f Collapse all folders from the sidebar header

### v0.3.142 — 2026-09-28T18:12:21-05:00 (`805601e`)

- **[fix]** `0dcc59a` · 2026-09-28T17:31:20-05:00 · **Command+C copies Codex's own selection** (`fix/codex-pane-copy-20260928`)
  - Notes: In a Codex pane, drag over text, press Command+C: Codex says Copied N chars and the text pastes in other apps
  - Pins: `electron/src/renderer/terminal-keys.test.ts::sends ⌘C to a program that owns the pointer and reads kitty key reports`, `electron/src/renderer/terminal-replay-buffer.test.ts::restores Codex's kitty keyboard flags on the screen that pushed them`
- **[feature]** `17ca80d` · 2026-09-28T17:12:51-05:00 · **Browser is a pane in the split, like every other pane** (`feat/browser-pane-leaf-20260928`)
  - Notes: Pane + > Browser opens a Browser pane beside it; its page survives splits, tidy, maximize and workspace switches, and reopens after restart
  - Pins: `electron/src/renderer/terminal-workspace.test.ts::is a pane of its own beside the source, and no terminal`, `electron/src/renderer/terminal-workspace.test.ts::keeps its page through a save, and survives the host roster reconciling`
- **[record]** `c761aa2` · 2026-09-27T13:00:20-05:00 · **Record the finished cleanup and the decisions left for the owner** (`record/review-20260927-final`)
- **[fix]** `b6dd78f` · 2026-09-27T12:59:42-05:00 · **Phone relay reconnects back off; iPhone lock revokes over its proven route** (`land/review-20260927-r12`)
  - Notes: A relay that stays down is retried at 3, 6, 12, 24 then 30 s; a session that reached it retries in 3 s
  - Pins: `crates/interface-remote/src/http/direct_tests.rs::relay_retries_back_off_to_thirty_seconds_and_restart_after_a_session`
- **[internal]** `2672322` · 2026-09-27T12:54:34-05:00 · **Host collaboration and orchestration split; packet and transcript rules move into their crates** (`land/review-20260927-r26`)
  - Notes: The collaboration packet builder moves into its crate; the host module splits; Orchestration's transcript rules move into their crate; the host module splits; One registry access module and one push module serve every host owner; Owners share one payload module and dispatch on CHANNELS; Setting changes, synthetic events and kept cleanups each build once; A failed host startup closes what it opened through one teardown; Provider resolvers, path guards and Codex overrides are written once
- **[internal]** `b7e0652` · 2026-09-27T12:36:26-05:00 · **xtask: one checked-file open, gate runner module, worktree parser, typed promotion phases** (`land/review-20260927-r28`)
  - Notes: One checked-file open for release and packaging; digests format themselves; Gate execution leaves the packaging module; both apps pass one check list; Every xtask git child gets a transfer deadline; one worktree parser; Promotion phases are typed; one promotion lock, report and git fixture
- **[internal]** `9d3a4e2` · 2026-09-27T12:27:30-05:00 · **Host terminal dispatch split into arms; one module owns capture generations** (`land/review-20260927-r25`)
  - Notes: Host terminal helpers are written once; Pin the terminal dispatch's authority refusals; Terminal dispatch arms move into their own modules; Terminal authority refusals and desktop errors are built once; One module owns the capture-generation layout; Claude session lookup splits into protocol, receipts, registry and probes
- **[speed]** `f9ecde2` · 2026-09-27T12:24:04-05:00 · **Terminal signals and liveness checks without /bin/kill forks** (`land/review-20260927-ct8`)
  - Notes: Closing a shell pane spawns one process instead of four
- **[fix]** `8a22c47` · 2026-09-27T12:11:38-05:00 · **Checkpoints flush the drive twice instead of per file; release binaries report the product version** (`land/review-20260927-r35`)
  - Notes: A 40-file turn checkpoint takes ~15 ms instead of ~200 ms; iface --version prints the release version
  - Pins: `apps/iface/src/app_tests.rs::version_prints_the_product_version_without_touching_state`
- **[fix]** `505a8f6` · 2026-09-27T11:35:11-05:00 · **Terminal create and restart share one spawn path; service.rs split; bindings follow one rule** (`land/review-20260927-r24`)
  - Notes: A restart and a create of the same pane produce identical entries; a cwd-changed spawn keeps its lease until teardown
  - Pins: `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_failed_restart_undoes_each_spawn_step`
- **[internal]** `46d1381` · 2026-09-27T11:32:33-05:00 · **Two more load-flaky tests fixed: Codex start-error race, Qwen trap order** (`land/review-20260927-flaky2`)
  - Notes: Two more load flakes: Codex start-error wording race, Qwen trap order
- **[fix]** `050212f` · 2026-09-27T11:15:15-05:00 · **Harness and control desk stop refusing every write when full; Stop always records** (`land/review-20260927-r32`)
  - Notes: A full control history still records Stop and deadline expiry; harness drops its oldest resolved items
  - Pins: `crates/interface-collaboration/src/voice_control.rs::a_full_history_still_records_stop_and_interruptions_in_the_reserved_room`
- **[internal]** `e8b3c74` · 2026-09-27T10:50:52-05:00 · **iface TUI split into reducer, approval, collaboration, orchestration and terminal modules** (`land/review-20260927-r27`)
  - Notes: Split the iface TUI into reducer, approval, collaboration, orchestration and terminal modules; One orchestration agent listing, a NewSession for starts, and a composer key table; Shared iface CLI parsers and smaller command helpers
- **[internal]** `334950b` · 2026-09-27T10:44:53-05:00 · **Agent policy and desktop service split into modules; one terminal-state lock** (`land/review-20260927-r23`)
  - Notes: Split the agent approval policy into modules; Agent policy tables share one list per family; Saved terminal roster and restart authority share one lock; Desktop service state machines move into their own modules; Workspace operations share one authority prologue; ACP profile saves apply one set of field rules; Keep agent_policy.rs as the module root with its tests inline
- **[fix]** `7039c51` · 2026-09-27T10:24:58-05:00 · **iface back to 0.1.169: Ctrl+Y to scrollback, --continue opens the newest, full scrolling** (`land/review-20260927-r33`)
  - Notes: In fullscreen iface, Ctrl+Y writes the transcript to native scrollback and --continue opens the newest session
  - Pins: `apps/iface/tests/tui_pty.rs::ctrl_y_writes_the_fullscreen_transcript_to_native_scrollback`
- **[speed]** `4dcdf8b` · 2026-09-27T10:05:51-05:00 · **Window focus stops launching provider CLIs; Codex listing reused for a minute** (`land/review-20260927-r29`)
  - Notes: Focusing the window with unchanged provider binaries spawns no probe; Refresh still re-probes
- **[speed]** `bfa5767` · 2026-09-27T10:02:33-05:00 · **Usage reads once a minute after turns; live logs compact to 90%; drag refits throttled** (`land/review-20260927-e18`)
  - Notes: A 24-frame splitter drag beside four terminals refits 12 times, not 92
- **[fix]** `610c098` · 2026-09-27T10:01:01-05:00 · **Ambiguous sends are never offered for retry; Make orchestrator keeps the lead's approvals** (`land/review-20260927-e17`)
  - Notes: A Claude or Codex send whose delivery is unknown shows Resend/Discard instead of a retry
  - Pins: `electron/src/main/rust-ipc-bridge-error-shape.test.ts::classifies a pre-change rejection, without the shape, as unknown`
- **[fix]** `1b57f0e` · 2026-09-27T09:55:09-05:00 · **ACP approvals and Stop reach a parked turn; iface exec and composer stay fail-closed** (`land/review-20260927-r31`)
  - Notes: Answering or stopping a Grok/Qwen/ACP permission prompt works while its send is in flight
  - Pins: `crates/interface-runtime/tests/runtime.rs::approval_and_cancel_reach_a_send_parked_on_its_turn`
- **[internal]** `517d244` · 2026-09-27T09:52:16-05:00 · **One basename, glyph set, chrome observer and usage-tone rule; Composer CSS declared once** (`land/review-20260927-e12x`)
  - Notes: One basename, context-percent, coordinator and usage-tone rule in the renderer; One glyph set and one chrome observer for the transcript and subagent panel; CLI tool rows share one rule; Composer.css declares each selector once
- **[internal]** `ce316f4` · 2026-09-27T09:50:26-05:00 · **Two flaky Rust tests fixed: PTY drain in sleep tests, Codex reap waits for the fake** (`land/review-20260927-ptyfix`)
  - Notes: Drain the PTY in the workspace-sleep process tests so the gate cannot hang; Wait for the fake Codex to start in the reap test instead of 750 ms
- **[internal]** `eebbcbc` · 2026-09-27T08:58:54-05:00 · **Terminal port handshake in its own tested controller; typed preload channels; tested dev scripts** (`land/review-20260927-e13`)
- **[internal]** `badcfb1` · 2026-09-27T08:58:32-05:00 · **Collaboration MCP server with one connection struct; one UTF-8 clipper** (`land/review-20260927-r22`)
  - Notes: Pin the collaboration MCP server's frames before refactoring; One connection struct and one spawn arm in the collaboration MCP server; One byte clipper; no panicking lookups in the Away cycle; one id rule
- **[fix]** `e860858` · 2026-09-27T08:55:39-05:00 · **One push-channel set and turn-ended rule; terminal activity rules shared with Rust** (`land/review-20260927-e11`)
  - Notes: xterm's own protocol replies no longer clear a pane's waiting state; both suites read one activity fixture
  - Pins: `electron/src/renderer/terminal-activity.test.ts::distinguishes focus protocol reports from actual terminal input`
- **[internal]** `7fb8325` · 2026-09-27T08:50:25-05:00 · **Collaboration rules as one statement each; one drift check and effect ladder** (`land/review-20260927-r21`)
  - Notes: Pin collaboration record bytes and first-failure messages before refactoring; Write each collaboration validation rule as one invalid_if! statement; One approval-drift check and one effect ladder in the collaboration coordinator
- **[internal]** `4f58c94` · 2026-09-27T08:02:11-05:00 · **Worktree creation in phases; one project walk for Quick Open and search** (`land/review-20260927-r20`)
  - Notes: Pin worktree refusals, registry bytes and walk budgets before refactoring; Split worktree creation into phases; one ordered-map serializer; Call Unix metadata methods directly in storage; One project walk for Quick Open and search; one parent-segment helper
- **[internal]** `bcb12b7` · 2026-09-27T07:46:05-05:00 · **Contracts goldens, one prompt-context validator, one command table; preview grants only owning roots** (`land/review-20260927-r19`)
  - Notes: Pin the command catalog and prompt-context validation JSON; One struct for prompt-context validation and one command table; One function per prompt-context item and a plain estimate floor; One numstat walker, one discovery refusal path, tests out of the source tree; Output preview grants only roots that contain the target
- **[speed]** `35bee38` · 2026-09-27T07:40:59-05:00 · **Packaged app ships only electron-updater at runtime; updater loads after the window** (`land/review-20260927-e7`)
  - Notes: npm ls --omit=dev lists only electron-updater; background update checks never show an error box
- **[internal]** `ad9f079` · 2026-09-27T07:28:22-05:00 · **Runtime lifecycle and reducer admission helpers; runtime.rs split; a flaky discovery test fixed** (`land/review-20260927-r18`)
  - Notes: One lifecycle helper and one metadata projection in the runtime; Shared admission helpers in the orchestration reducer; One disabled path in the updater's startup decision; Split the runtime's cache writer, event pump and tests out of runtime.rs
- **[fix]** `2394cba` · 2026-09-27T07:14:44-05:00 · **One owner for each terminal stream's output; stream leaks on dismiss fixed** (`land/review-20260927-e16`)
  - Notes: A dismissed terminal leaves no output cursor, sleeping screen or native worker entry behind
  - Pins: `electron/src/renderer/terminal-stream-router.test.ts::ends every stream of a removed terminal and drops its later output`
- **[internal]** `49b1d9e` · 2026-09-27T07:05:00-05:00 · **Split the longest Codex, ACP and Claude session functions** (`land/review-20260927-r17`)
  - Notes: Split the long Codex, ACP and Claude session functions
- **[internal]** `89b53ac` · 2026-09-27T06:53:06-05:00 · **Codex and ACP share one transport and session core; workers share one helper set** (`land/review-20260927-r16`)
  - Notes: One transport and one session core for Codex and ACP; OpenCode and Antigravity workers use the shared worker helpers; One checked transcript read and one blocking-store helper; Shared collaboration test fakes; no fixed sleep in a Codex exit test; Keep the shared transport's internals private
- **[speed]** `eb0cda8` · 2026-09-27T06:50:16-05:00 · **Chat panes split into transcript and composer: typing no longer redraws the transcript** (`land/review-20260927-e15`)
  - Notes: Five keystrokes in a chat pane render the composer only; streamed tokens never render the composer
- **[internal]** `b4d88d0` · 2026-09-27T06:20:42-05:00 · **Shared lock, uid, timestamp, digest and path helpers replace per-crate copies** (`land/review-20260927-r15`)
  - Notes: Share the helpers every crate kept its own copy of; One credential-name check for the gate cache and navigation
- **[fix]** `d858562` · 2026-09-27T05:58:42-05:00 · **One commit protocol per store crate; a write that reached nothing no longer locks a store** (`land/review-20260927-r14`)
  - Notes: An oversized harness result is refused and the next save succeeds; drift still closes the store
  - Pins: `crates/interface-settings/src/harness.rs::an_oversized_result_is_refused_and_later_records_still_save`
- **[internal]** `fcede82` · 2026-09-27T05:50:39-05:00 · **App.tsx loses 841 lines: send lane, settings tokens, history paging, orchestration, inspector** (`land/review-20260927-e14`)
  - Notes: Move the provider send lane and steering queue into a tested OutboundLane; Give the settings attempt-token protocol one owner; Move history paging and event batching out of App; Settings reads its customization inventory through an API prop; Move orchestration state and the canvas intents out of App; Move the task harness and inspector wiring out of App; Keep the removed-resume guard on the outbound lane file
- **[internal]** `06f047f` · 2026-09-27T05:26:08-05:00 · **Remove Rust seams only tests reached: roster-only session binding, nested-process hook** (`land/review-20260927-r13`)
  - Notes: Drop the roster-only provider session seam nothing calls; Drop the nested-process hook the terminal service never calls; Drop test-only collaboration decoder and workers models()
- **[fix]** `6362304` · 2026-09-27T05:11:14-05:00 · **iface worker broker and agent socket survive bad peers, oversize replies and turn races** (`land/review-20260927-r11`)
  - Notes: A dropped client before accept leaves the workers broker serving; an oversized agent reply gets an error frame
  - Pins: `apps/iface/src/workers/transport_tests.rs::a_peer_that_hangs_up_before_accept_does_not_stop_the_broker`
- **[internal]** `3d62c9a` · 2026-09-27T05:08:30-05:00 · **Remove unreachable terminal kill, rename, restart and duplicate paths** (`land/review-20260927-e10`)
  - Notes: Drop a dead agent panel prop, a copied edge mapping and a test-only eof event
- **[fix]** `f6cfe0f` · 2026-09-27T04:56:23-05:00 · **Hidden notices stay hidden; control desk survives blocked storage; unsafe reopens are not retried** (`land/review-20260927-e9`)
  - Notes: The Fast toggle notice is read aloud but not drawn; a non-transient reopen refusal goes straight to the shell fallback
  - Pins: `electron/src/renderer/terminal-reopen-retry.test.ts::does not retry a refusal whose code is not transient, and goes straight to the shell fallback`
- **[internal]** `787ed6a` · 2026-09-27T04:49:10-05:00 · **Gate receipts per suite, timed steps, a lighter pin guard and a current terminal smoke** (`land/review-20260927-r9`)
  - Notes: Pin guard reads only the pinned files a push changes; Gate prints each step's time and each receipt keeps its own suite's; Each gate receipt is keyed by what its own suite reads; Terminal smoke expects OpenCode's deliberate --auto launch
- **[fix]** `3212696` · 2026-09-27T04:39:17-05:00 · **Launch in the saved theme without a light repaint** (`land/review-20260927-e6`)
  - Notes: Launching in a dark or palette theme paints that theme first, with one terminal re-theme
  - Pins: `electron/src/renderer/appearance.test.ts::paints ${theme} before mount and never repaints anything else on the way to the confirmed read`
- **[speed]** `333c0f4` · 2026-09-27T04:21:50-05:00 · **Agent page, remote access and phone polls pause while hidden or settled** (`land/review-20260927-e5`)
  - Notes: A hidden agent page and a closed Settings make no work or remote-access reads
- **[fix]** `31987ea` · 2026-09-27T04:14:00-05:00 · **worktree-prune keeps the Dev and promote checkouts; exact gate scope and pin spans** (`land/review-20260927-r8`)
  - Notes: cargo xtask worktree-prune (dry run) lists ~/Developer/.interface-dev and .interface-promote as kept
  - Pins: `xtask/src/worktree_prune.rs::the_promote_driver_is_kept_as_itself`
- **[internal]** `f77d876` · 2026-09-27T03:59:32-05:00 · **Electron gate in about 100 s: pooled renderer lanes with per-lane deadlines** (`land/review-20260927-e8`)
  - Notes: Run the renderer gate lanes side by side, each with its own deadline
- **[speed]** `b2413ab` · 2026-09-27T03:44:58-05:00 · **Control desk, collaboration and Away polls copy only what they return** (`land/review-20260927-r7`)
  - Notes: The control-desk view is byte-identical to the old clone-then-null view
- **[speed]** `9955156` · 2026-09-27T03:34:31-05:00 · **Terminal requests stay off the lifecycle lock and disk writes; worktree ledger off async workers** (`land/review-20260927-r6`)
  - Notes: A terminal request returns while a lifecycle publish is stalled in its durable write
- **[fix]** `213eb05` · 2026-09-27T03:05:25-05:00 · **iface TUI stays live while providers start, send, stop and resume** (`land/review-20260927-r5`)
  - Notes: Ctrl+C during a slow provider start in iface exits at once; a second Enter during a send sends once
  - Pins: `apps/iface/tests/tui_pty.rs::ctrl_c_during_a_slow_provider_start_exits_without_waiting_for_it`
- **[speed]** `51a9511` · 2026-09-27T02:47:29-05:00 · **Sidebar, palette and open Browsers redraw only rows that changed** (`land/review-20260927-e4`)
  - Notes: A streamed token or rename redraws no other sidebar row; palette Enter runs the highlighted command after a push
- **[fix]** `1a16134` · 2026-09-27T02:23:40-05:00 · **Busy Codex/ACP sessions survive output bursts; claude after codex in a pane is captured** (`land/review-20260927-r10`)
  - Notes: A burst of 40+ small frames while a Codex session is busy is delivered in order instead of failing the session
  - Pins: `crates/engine-codex/src/transport.rs::a_burst_while_the_session_is_busy_waits_instead_of_failing_the_transport`
- **[record]** `16b2876` · 2026-09-27T02:08:35-05:00 · **Record the cleanup review and its landing lanes** (`record/review-20260927-todo`)
- **[speed]** `a5e5a95` · 2026-09-27T02:07:51-05:00 · **Terminal panes redraw only what changed: titles, activity and splitter drags** (`land/review-20260927-e3b`)
  - Notes: A background pane's title change or a splitter step redraws no other pane or emulator
- **[speed]** `18de876` · 2026-09-27T02:03:20-05:00 · **Provider CLIs are asked their version once per install; consultations probe one provider** (`land/review-20260927-r3`)
  - Notes: A second provider probe with the same Claude or Codex binary spawns no --version process
- **[internal]** `f9c4c8f` · 2026-09-27T01:58:58-05:00 · **Terminal cursor fixture waits for layout before clicking (fixes a flaky gate lane)** (`land/fixture-cursor-race`)
  - Notes: Wait for the cursor fixture's screens to lay out before clicking
- **[speed]** `1b34694` · 2026-09-27T01:31:17-05:00 · **Window focus no longer rewrites the session registry when nothing changed** (`land/review-20260927-r2`)
  - Notes: Focusing the window with no new sessions leaves registry.json's revision, inode and bytes unchanged
- **[speed]** `5cfee06` · 2026-09-27T01:19:26-05:00 · **Agent tool calls skip ps walks; sleeping panes and the budget watch wake less** (`land/review-20260927-r4`)
  - Notes: A Claude or Codex tool call in a pane with no waiting worker result runs no ps process
- **[speed]** `bb0ac93` · 2026-09-27T01:12:30-05:00 · **Streaming chats redraw only what changed: rows, markdown and hidden panes** (`land/review-20260927-e1e2`)
  - Notes: A long tool-heavy chat streams without re-rendering its folded runs; markdown parses only the growing tail
- **[fix]** `9d1248c` · 2026-09-27T00:44:46-05:00 · **Agent store errors no longer close chats; fewer locks and thinking signals per token** (`land/review-20260927-r1`)
  - Notes: An unreadable agent store leaves a plain Codex chat open, and a reasoning span sends one thinking signal
  - Pins: `apps/interface-host/src/agents_tests.rs::an_unreadable_agent_store_never_closes_an_ordinary_chat`
- **[feature]** `1194f46` · 2026-09-27T00:17:02-05:00 · **Pane + drops Canvas; Browser opens in a pane; iOS Simulator starts an iPhone** (`fix/new-pane-menu-20260926`)
  - Notes: Pane + has no Canvas; Browser opens your running dev server in a pane beside the terminals; iOS Simulator opens on an iPhone
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::opens a New pane menu from the + with tools, launchers and only real chords`, `electron/src/main/browser-pane.test.ts::pins a webview's preferences whatever its markup asked for`, `crates/interface-terminal/src/workspace_tools.rs::prefers_a_booted_iphone_then_the_last_used_then_the_newest_runtime`
- **[record]** `9410a57` · 2026-09-26T22:51:13-05:00 · **Record the 0.3.141 phone release** (`docs/phone-0.3.141-20260926`)

### v0.3.141 — 2026-09-26T22:47:36-05:00 (`dea6205`)

- **[fix]** `d257bf9` · 2026-09-26T22:43:34-05:00 · **Chat no longer lists Away work and review sessions** (`fix/chat-hide-away-runs-20260926`)
  - Notes: Chat's session list has no 'Away work' or 'Away review' rows; the Away panel still opens them
  - Pins: `electron/src/renderer/away-view.test.ts::lists the sessions each Away turn created and skips turns still launching`
- **[feature]** `8a3a65f` · 2026-09-26T22:37:06-05:00 · **iPhone reaches the Mac over Tailscale and keeps its routes current** (`feat/phone-tailscale-route-20260926`)
  - Notes: With Tailscale on both devices the paired iPhone connects on cellular and shows Connected through Tailscale
  - Pins: `crates/interface-contracts/src/remote_connection.rs::mac_routes_add_tailnet_addresses_without_widening_the_qr`, `apps/interface-ios/MacAddressTests.swift::testATailnetRouteFromTheMacConnectsAndNamesItself`

### v0.3.140 — 2026-09-26T22:28:41-05:00 (`21180fa`)

- **[feature]** `23fbaa8` · 2026-09-26T22:26:27-05:00 · **iPhone session list grouped by project, matching the Mac sidebar** (`feat/phone-projects-20260926`)
  - Notes: The iPhone lists sessions under the same projects as the Mac sidebar, with Other last
  - Pins: `electron/src/remote/projects.test.ts::files each terminal under the same project as the Rust phone roster`, `crates/interface-remote/src/workspaces.rs::files_terminals_under_the_same_projects_as_the_mac_sidebar`, `apps/interface-ios/MacAddressTests.swift::testRosterV3FilesSessionsLikeTheMacSidebar`
- **[fix]** `04e96e8` · 2026-09-26T22:06:04-05:00 · **iPhone pairing works on a restored or transferred iPhone** (`fix/phone-restored-identity-20260926`)
  - Notes: On an iPhone set up from an older one, scanning a Mac pairing code shows a verification number instead of 'no longer approved'
  - Pins: `apps/interface-ios/MacAddressTests.swift::testAMissingIdentityIsNotAMacRefusal`
- **[look]** `9daadb6` · 2026-09-26T21:53:29-05:00 · **iPhone app refresh: provider marks, reply-bar glyph fix, phone 0.3.2** (`feat/phone-refresh-20260926`)
  - Notes: Phone session rows and the reply bar show each provider's mark; Safari's Add to Home Screen uses the Fold icon
  - Pins: `crates/interface-remote/src/http.rs::serves_the_touch_icon_and_refuses_a_build_without_it`, `electron/src/remote/codicons.test.ts::names only codicons the vendored subset draws`
- **[fix]** `7b5ba63` · 2026-09-26T21:21:31-05:00 · **Terminal text keeps its bold GPU weight after sleep or a display change** (`fix/terminal-webgl-recovery-20260926`)
  - Notes: Pane text stays bold after sleep/wake or a display change instead of turning thin until a workspace switch
  - Pins: `electron/src/renderer/terminal-webgl-pool.test.ts::hands a freed slot to a visible pane left on the DOM renderer`, `electron/src/renderer/terminal-webgl-pool.test.ts::backs off, gives up, and starts over once a renderer has stayed up`, `electron/src/renderer/terminal-webgl-pool.test.ts::brings a visible pane back to WebGL instead of leaving its text thin`
- **[look]** `0561f38` · 2026-09-26T14:04:24-05:00 · **Compact top bar: Layout, Agent overview, Usage and Attention in ···** (`feat/compact-toolbar-20260926`)
  - Notes: The top bar shows only Outputs, Voice, Away and ···; ··· lists Attention, Usage and the layout controls
  - Pins: `electron/src/renderer/components/AttentionTray.test.ts::draws no bell of its own; the ··· button carries its count and says it`, `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps the reference layout rows, now inside the window's ··· menu`, `electron/src/renderer/workspace-ui.test.ts::shows the update pill beside the terminal controls only while an update exists`
- **[look]** `0a4b7ae` · 2026-09-26T12:38:28-05:00 · **Porcelain: the new default look** (`feat/porcelain-20260926`)
  - Notes: Panes are light frames around dark screens; Settings > Color theme lists Porcelain and Porcelain Night
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::draws a rounded selection box around the focused terminal pane`, `electron/src/renderer/theme-accessibility.test.ts::keeps terminal body, idle and focused header tiers distinct in every palette`
- **[feature]** `cfce6be` · 2026-09-26T09:51:00-05:00 · **Agent permission modes: Ask, provider Auto or Bypass per agent** (`feat/agent-permission-modes-20260925`)
  - Notes: Agent Advanced settings offer Ask, Auto or Bypass; Auto and Bypass agents run in Claude's or Codex's own mode and send what they escalate to you
  - Pins: `crates/engine-claude/src/adapter_tests.rs::agent_access_sessions_leave_every_escalation_to_interface`, `crates/engine-codex/src/adapter_tests.rs::agent_access_threads_use_codex_auto_review_or_full_bypass`, `apps/interface-host/src/agents_tests.rs::approvals_follow_the_mode_the_live_session_launched_with`, `crates/interface-settings/src/agents/tests.rs::permission_modes_live_beside_the_roster_and_follow_the_agent`
- **[record]** `05b70fd` · 2026-09-25T22:10:18-05:00 · **Record staged Claude restart repair** (`fix/claude-reopen-20260925`)
  - Notes: Record verified 0.3.138 staging and production acceptance boundary
- **[internal]** `2920b0a` · 2026-09-25T22:08:32-05:00 · **Reconcile Claude fix release into Dev** (`fix/claude-reopen-20260925`)
  - Notes: Claude conversations reopen after a project moves; chore(release): bump Interface to 0.3.138
- **[record]** `8a89bce` · 2026-09-25T21:27:48-05:00 · **Record Claude restart fix release readiness** (`fix/claude-reopen-20260925`)
  - Notes: Record verified Claude restart fix and production release boundary

### v0.3.139 — 2026-09-26T21:05:28-05:00 (`3726446`)

- **[look]** `80a2fea` · 2026-09-26T21:00:59-05:00 · **Original pane buttons: smaller, thicker +, ···, expand and close** (`fix/pane-buttons-20260926`)
  - Notes: Pane header buttons (···, expand, +, close) are small 12px glyphs with a heavier stroke again
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps tabs mounted in explicit tabpanels and labels close as detach-only`, `electron/src/renderer/components/AgentTile.test.ts::keeps the pane controls visible and offers a split`
- **Left on Dev:** ab6d914 Agent permission modes: Ask, provider Auto or Bypass per agent; 784e597 Porcelain: the new default look; c353b8f Compact top bar: Layout, Agent overview, Usage and Attention in ···

### v0.3.138 — 2026-09-25T22:00:27-05:00 (`d76288e`)

- **[fix]** `cbedd8f` · 2026-09-25T21:26:25-05:00 · **Claude conversations reopen after a project moves** (`fix/claude-reopen-20260925`)
  - Notes: Restart Interface after moving a project; saved Claude panes reopen their original conversations.
  - Pins: `apps/interface-host/src/claude_terminal_session_tests.rs::moved_project_receipt_recovers_on_restart_and_unblocks_the_saved_pane`

### v0.3.137 — 2026-09-25T18:54:25-05:00 (`b1399ee`)

- **[look]** `0919a3a` · 2026-09-25T18:18:35-05:00 · **Blue dot when an agent is done** (`feat/done-dot-20260925`)
  - Notes: An agent that finishes while you're in another pane shows a blue dot until you focus it
  - Pins: `electron/src/renderer/components/TerminalPrimitives.test.ts::paints a finished, unseen agent turn blue until something outranks it`, `electron/src/renderer/components/Sidebar.test.ts::paints a workspace blue when an agent finished a turn nobody has looked at`, `electron/src/renderer/terminal-activity.test.ts::marks a finished turn in a pane nobody is looking at, never the focused one`
- **[fix]** `109f6bf` · 2026-09-25T15:15:51-05:00 · **Explain panes waiting on iCloud startup files** (`fix/icloud-startup-wait-20260925`)
  - Notes: A new pane in an iCloud project with cloud-only git or CLAUDE.md files shows 'Waiting for cloud download: …' until they download, not a blank pane
  - Pins: `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_pane_names_an_evicted_startup_file_until_it_downloads`, `crates/interface-terminal/src/startup_files.rs::a_git_directory_fetches_head_config_and_then_the_branch_ref`, `electron/src/renderer/terminal-startup-download.test.ts::matches the Rust projection and its notice`
- **[fix]** `c801a21` · 2026-09-25T14:50:07-05:00 · **Launch security fixes: scoped agent reads, sandboxed Claude Away, confined pane/phone access** (`fix/security-sweep-20260925`)
  - Notes: Agents ask before reading outside their folders; Claude Away runs sandboxed; read_pane and the phone listener refuse outside callers
  - Pins: `crates/interface-desktop-services/src/agent_policy.rs::policy_table`, `crates/engine-claude/src/adapter_tests.rs::workspace_write_launches_claude_in_its_os_sandbox`, `crates/interface-collaboration/src/project_away/tests.rs::claude_runs_sandboxed_without_bypass_and_codex_keeps_its_sandbox`, `crates/interface-remote/src/http/direct_tests.rs::only_local_network_peers_reach_the_nearby_listener`
- **[feature]** `b92414d` · 2026-09-24T17:42:18-05:00 · **Every release lists what it ships and leaves out** (`feat/release-contents-20260924`)
  - Notes: promote --dry-run prints what ships, what stays on Dev, and what it replaces
  - Pins: `xtask/src/landings.rs::release_summary_groups_user_facing_landings_and_counts_the_rest`, `xtask/src/closeout.rs::closeout_records_what_a_landing_is_and_guards_shipped_pins`
- **[record]** `99b1882` · 2026-09-24T17:18:30-05:00 · **Design for release contents and shipped pins** (`docs/release-contents-20260924`)
- **[feature]** `edbb49d` · 2026-09-24T16:13:05-05:00 · **Record the icon review handoff** (`feat/icon-review-20260924`)
- **[feature]** `c3207ec` · 2026-09-24T16:11:41-05:00 · **Consistent icon weights and one glyph per meaning** (`feat/icon-review-20260924`)
- **[feature]** `9977dc7` · 2026-09-24T15:44:45-05:00 · **Slide the sidebars and animate every floating surface** (`feat/sidebar-motion-20260924`)
  - Notes: Slide the output preview in and out like the agent overview

### v0.3.135 — 2026-09-24T15:27:17-05:00 (`1916dcc`)

- **[internal]** `a8b1054` · 2026-09-24T15:23:54-05:00 · **Record completed Agent production test** (`test/agent-production-sandbox-20260924`)
  - Notes: Mark sandboxed production test complete and record Dev verification
- **[internal]** `a2955b3` · 2026-09-24T15:22:22-05:00 · **Sandboxed Agent production verification** (`test/agent-production-sandbox-20260924`)
  - Notes: Isolate production package tests from owner state; Preserve Agent stop reasons and verify sandboxed release behavior; Permit read-only terminal identity checks in production sandbox tests; Record passing signed Agent production acceptance
- **[feature]** `7e66acc` · 2026-09-24T15:10:09-05:00 · **Preview project outputs beside agent terminals** (`feat/output-preview-20260924`)
  - Notes: Add output previews and file-specific feedback
- **[fix]** `c34d064` · 2026-09-24T14:49:31-05:00 · **Verify Keep Downloaded for Claude startup repair** (`fix/claude-startup-hang-20260924`)
  - Notes: docs: verify Finder pin for Claude startup repair
- **[fix]** `a8edeff` · 2026-09-24T14:45:00-05:00 · **Record Claude startup iCloud repair** (`fix/claude-startup-hang-20260924`)
  - Notes: docs: record Claude startup iCloud repair
- **[fix]** `08916b4` · 2026-09-24T12:00:12-05:00 · **Restore saved panes after startup retries** (`fix/blank-saved-panes-20260924`)
  - Notes: Keep saved pane reopen retries alive across workspace updates

### v0.3.134 — 2026-09-24T10:29:57-05:00 (`c53970b`)

- **[fix]** `c0ade9b` · 2026-09-24T10:26:45-05:00 · **Blue terminal typing caret** (`fix/reference-terminal-caret-20260924`)
  - Notes: Match the terminal typing caret to the blue reference
- **[record]** `430b8da` · 2026-09-24T09:52:47-05:00 · **Record the 0.3.133 promotion** (`docs/promoted-0.3.133`)
  - Notes: Record the 0.3.133 promotion in the to-do list

### v0.3.133 — 2026-09-24T09:48:19-05:00 (`7bd2948`)

- **[feature]** `82da35a` · 2026-09-24T09:45:47-05:00 · **Record Agent mode validation and Dev landing** (`feat/agent-mode-audit-20260924`)
- **[feature]** `30516fe` · 2026-09-24T09:43:35-05:00 · **Reliable Agent tasks and calendar routines** (`feat/agent-mode-audit-20260924`)
  - Notes: Make Agent tasks durable, scoped and verifiable
- **[record]** `3fce3a9` · 2026-09-24T08:22:03-05:00 · **Record performance audit verification** (`improve/performance-audit-20260924`)
  - Notes: Record completed audit verification and Dev landing
- **[speed]** `48bae58` · 2026-09-24T08:19:53-05:00 · **Performance audit fixes and controller cleanup** (`improve/performance-audit-20260924`)
  - Notes: Implement performance audit fixes and controller boundaries
- **[fix]** `bf96971` · 2026-09-24T07:13:07-05:00 · **Saved panes reopen after their folder moves** (`fix/reopen-moved-folder`)
- **[fix]** `cf45137` · 2026-09-23T23:03:58-05:00 · **Faster workspace switching** (`fix/workspace-switch-latency-20260923`)
  - Notes: Reuse bounded terminal GPU renderers across workspace switches
- **[record]** `612af01` · 2026-09-23T22:23:39-05:00 · **Note symlinked-project terminal fix is ready to promote** (`docs/symlink-launch-ready`)
- **[record]** `e00618a` · 2026-09-23T22:23:10-05:00 · **Record the thin terminal cursor as ready to promote** (`docs/caret-ready-20260923`)
- **[fix]** `8881d95` · 2026-09-23T22:20:58-05:00 · **Launch terminals in projects moved behind a symlink** (`fix/symlinked-workspace-launch`)
- **[fix]** `6e2399f` · 2026-09-23T22:17:43-05:00 · **Thin caret is the default terminal cursor** (`fix/default-caret-20260923`)

### v0.3.132 — 2026-09-23T21:43:58-05:00 (`c1b76c3`)

- **[internal]** `c5ef15b` · 2026-09-23T20:21:11-05:00 · **Project and Dev checkout out of iCloud** (`chore/project-off-icloud`)
  - Notes: Move the project and Dev checkout out of iCloud
- **[fix]** `ba4f931` · 2026-09-23T17:05:37-05:00 · **Thinner, shorter bar cursor in terminal panes** (`fix/smaller-terminal-cursor`)
- **[record]** `2c383ae` · 2026-09-23T14:48:19-05:00 · **Close the agent overview entry after its 0.3.131 promotion** (`docs/agent-overview-accepted-20260923`)
- **[internal]** `3d556b6` · 2026-09-23T14:37:13-05:00 · **Audit cleanup (host 2/3): readiness-based terminal I/O, one service ticker, journal off workers** (`land/audit-host-b2`)
  - Notes: Spell out persisted orchestration error codes; derive permit count; Log why remote access is unavailable instead of failing silently; Read the hook's own parent and liveness without spawning ps or kill; Give host.sock request reads the client's 15 s deadline; Bound MCP socket lines and in-flight calls; unlink only our sockets; Kill the Antigravity usage probe's whole process group on timeout; Retire input lanes and replay streams with their terminals; Drive fd3/fd4 by readiness and encode frames outside the queue lock; Commit orchestration mutations on the blocking pool; Run collaboration journal transitions on the blocking pool; Read one phone draft or twenty results without cloning the harness; Parse only appended Codex index lines; back off missing prompts; Start the login-shell PATH probe before the stores open; Cheaper workspace-sleep checks and a backoff after a declined sleep; Drop back-to-back process listings from pane caller checks; Pin a renamed pane's title without holding the state lock; One service ticker for sleep, shell-display and Grok-title ticks; Give release_terminal_entries a per-entry function; drop a dead Option; Extract the process waiter and one teardown retry loop; Handle a reaped manual provider in one branch; Name the capture ownership guard, live state and capture reset once; Use is_live everywhere; a bare Mutex<bool> for the provider wake; Share the Claude capture lock release and trust-boundary parsing; Share the create/restart spawn exposure tail; Deduplicate uncommitted-terminal cleanup and back off its retries

### v0.3.131 — 2026-09-23T14:36:58-05:00 (`af0895a`)

- **[internal]** `4d366d8` · 2026-09-23T14:30:11-05:00 · **Audit cleanup (renderer 2/6): memo boundaries restored; renderer loads while the host starts** (`land/audit-renderer-b2`)
  - Notes: Delete two phone conversation rules that never matched; Delete superseded delegation fixtures; document the HPKE vector generators; Run the CLI pane appearance matrix once per style, not three times; Read orchestration and collaborations once at launch, not twice; Forbid renderer workers in the CSP now that Monaco is gone; Read terminal bells once, from the PTY scanner only; Keep memo(IntegratedTerminalCenter) effective: hoist four inline props; Keep memo(Sidebar) effective: stable project and workspace pin handlers; Skip sticky-header clip measurement while the sidebar is solid; Keep memo(AgentCanvas) effective across streamed frames and pushes; Skip the App render for voice level frames that light the same bars; Stop recompacting a live log whose disposable history still fits; Keep main-thread streaming incremental after a subagent spawns; Memoize the transcript's tool, file-change and subagent rows; Render session panes through a memoized SessionTile; Keep AgentCanvas picker options stable across pan and drag renders; Stop rewriting the saved roster for resize-only terminal upserts; Keep the workspace switcher stable through splitter drags and focus; Poll pane delegation at 1 s only while workers run or a turn is live; Release a parked terminal's WebGL context and reload it when slotted; Poll the phone's conversation and output only when they can change; Load the renderer while the Rust host starts
- **[internal]** `2dff636` · 2026-09-23T14:25:50-05:00 · **Audit cleanup (core 2/4): registry writes outside the lock; Away and ACP robustness fixes** (`land/audit-core-b2`)
  - Notes: Reject u64::MAX orchestration sequences instead of overflowing; Register for the ingress drain wakeup before checking the count; Bound the Claude fork parent walk so a parentUuid cycle ends; Open Claude transcripts with libc's O_NOFOLLOW, not magic numbers; Keep failed or oversize Claude history reads out of the LRU; Close Claude sessions against one shared deadline, aborting stragglers; Return Claude probe, usage and command answers before the CLI exits; Map Claude commands_changed through the shared command normalizer; Drop Claude context-usage refreshes that a newer value superseded; Shut one-shot ACP probe and usage agents down when the caller drops; Keep an approval answerable when its answer was definitely unsent; Decode ACP session/update params by moving fields, not via flatten; Share the Codex read-lane checkout and fail lanes on oversize frames; Clone and validate the orchestration snapshot once per apply; Answer missing project-root cwds with a stat, not a git spawn; Scan session-search pages on blocking threads inside each fetch task; Read collaboration history state and tails without whole-session copies; Write the registry cache after releasing the registry lock; Run the two registry cadence tests on tokio paused time; Document the registry save split in the area docs; Refuse a full Away history without marking storage uncertain; Keep the tail of long Away turn reports, where the result is; Let an owner's Away cycle Stop wait out the apply checkpoint; Delete the write-only collaboration audit_degraded flag; Stop re-driving collaborations that wait on a person per source token; Share one whole-registry push coalescer across the host's owners; Apply registry pushes with the event batch and keep unchanged rows; Format the merged orchestration start call
- **[internal]** `4ed2dd5` · 2026-09-23T14:12:16-05:00 · **Audit cleanup (services 2/5): iface transcript restyles only changes; batched review diffs** (`land/audit-services-b2`)
  - Notes: Look up saved terminals without cloning the whole roster; Skip rewriting an unchanged terminal workspace roster; Stop cloning the agent store for reads, idle ticks and text deltas; Skip the ps ancestry probe for MCP notifications in the worker bridge; Probe log-writer liveness only past the retention window; Avoid login-shell PATH lookups for overrides and optional helpers; Read the registry, not every provider, for unseen collaboration keys; Run steer-history and config fsyncs on the blocking pool; Harden settings store modes only when they are not already private; Remove stale registry rows in one pass after a complete listing; Read the managed-worktree store once when opening it; Parse the on-disk JSON in compare_and_set only when it changed; Use one terminal snapshot per remote request; bound history envelopes; Read prompt-context roots once per operation and realpath only distinct ones; Downscale pasted-image previews on the native buffer; Prefilter editor roots lexically before verifying them; Keep terminal-state gate reads off async workers in the host; Batch Review patch diffs into one git call per layer; Slow the iPhone roster poll while a session view is open; Probe nearby phone routes together and pass reply bodies through; Define container and tar tool families once in the agent policy; Lay out iface's transcript incrementally with exact wrapped heights
- **[record]** `b288753` · 2026-09-23T13:51:08-05:00 · **Record the agent overview's pending Dev acceptance** (`docs/agent-overview-dev-check-20260923`)
- **[fix]** `332a8d9` · 2026-09-23T12:47:39-05:00 · **Record the Claude activity landing awaiting the Dev refresh** (`fix/claude-background-activity`)
- **[fix]** `abf3a6b` · 2026-09-23T12:41:33-05:00 · **Claude panes stay working while background workflows run** (`fix/claude-background-activity`)
  - Notes: Keep Claude panes working while background workflows run
- **[record]** `4f0128c` · 2026-09-23T11:57:30-05:00 · **Record the pending Dev refresh after the instruction landing** (`docs/opus55-dev-refresh-handoff`)
- **[feature]** `34450ef` · 2026-09-23T11:04:24-05:00 · **Agent overview beside the terminal panes** (`feat/agent-overview-20260923`)
- **[record]** `401fc20` · 2026-09-23T10:50:41-05:00 · **Instruction files tuned for Opus 5.5** (`docs/opus55-instructions`)
- **[feature]** `d17faf0` · 2026-09-23T10:32:54-05:00 · **Integrate approved 0.3.130 with current Dev** (`feat/new-project-from-workspaces-20260922`)
  - Notes: Create projects from the Workspaces plus menu; Update packaged smoke for the Workspaces create menu; chore(release): bump Interface to 0.3.130
- **[internal]** `15b69ad` · 2026-09-23T08:52:29-05:00 · **Audit cleanup (host 1/3): no ps storms, batched terminal output, keystrokes off the lifecycle lock** (`cleanup/audit-host-20260923`)
  - Notes: Replace the unavailable collaboration control with an Option; Drop the write-only live-handle generation from the projection; Remove the redundant LocalHost mutation gate; Inline start_with_services and the agent_is_terminal alias; Test session management through the library, not a #[path] copy; Collapse the bootstrap opener structs into one runtime core; Open the worktree ledger in its own bootstrap helper; Delete the unreachable non-unix fallbacks in the host; Remove test-only constructor layers from the host API; Implement supplemental usage on the OpenCode reader directly; Drop the unreachable persist_provider_session lifecycle hook; Remove dead state from the OSC title scanners; Stop copying native worker output into an unreadable tail; Answer empty worker notices before walking the process tree; Stop re-reading an unchanged foreground job every second; Skip rewriting unchanged launch overlays at boot; Open the report-worker store without an unconditional rewrite; Restore phone access without rewriting unchanged settings; Stop busy-polling blocked output pumps every 2 ms; Merge queued PTY output into one frame and drop the pump timeout; Serve workspace sleep from one ticker instead of a thread per pane; Run hook and wrapper modes before building the Tokio runtime; Keep the committed orchestration snapshot as the read memo; Serve the owner window's orchestration calls without serde copies; Move path probes in async handlers off the Tokio workers; Adapt the lane to origin/dev's storage trim and iface lints; Build each collaboration status once and probe models together; Collect each approved session root once, without cloning rows; Keep a finished login shell's PATH when a background job holds stdout; Release the host read buffer after a huge pasted-image request; Read saved terminal cwds on the desktop worker, not the executor; Park the Mac control desk ticker while no run is live; Sleep the routine scheduler until work is due instead of every 2 s; Admit terminal input from a mirror, not the lifecycle lock
- **[internal]** `67355ae` · 2026-09-23T08:36:30-05:00 · **Audit cleanup (core 1/4): calmer registry saves and pushes; retired orchestrator DTOs removed** (`cleanup/audit-core-20260923`)
  - Notes: Remove retired orchestrator tool DTOs from contracts and TS mirror; Drop the dead iface_await long-running flag from the MCP server; Remove revoked broker bindings instead of flagging them; Delete Rust copies of TS-only contract helpers; Collapse test-only opencode worker entry points; Keep one ACP permission rule in validate_launch; Narrow engine-claude's public surface and drop dead variants; Trim engine-codex test-only helpers and unused root re-exports; Drop the unused engine_history accessor and shutdown key clones; Drop dead chrono and duplicate serde_json manifest entries; Save activity-only registry changes on a slow timer; Send timestamp-only registry pushes at most once a second; Build the Away inspect view without cloning the whole history; Clone only the last voice-control run for the display view; Read Away cycle controls and run once per Stop or feedback; Read delegate activity and terminal under one lock; End the audit peer-review hold on every exit; Load journal records without cloning each twice; Validate timestamps by shape then parse, without a round-trip; Cache Grok summaries and version across focus refreshes; Paint the cached registry before the first cold listing answers; Name the terminal arm in the registry cadence test; Remember a registry frame only once it was actually sent
- **[internal]** `356f2e2` · 2026-09-23T08:28:03-05:00 · **Audit cleanup (renderer 1/6): retired renderer code, dead mirrors and test-only API removed** (`cleanup/audit-renderer-20260923`)
  - Notes: Drop the orphaned dompurify override from electron/package.json; Trim dead code and a redundant xattr pass from dev-app.mjs; Delete two Electron tests that exercise no production code; Delete three unreferenced shared appearance and image declarations; Delete the retired TypeScript engine adapter contract; Delete orchestration mirror types that no TypeScript reads; Drop TerminalView's unread focused prop; Delete test-only terminal helpers and fix the switcher title docs; Delete the retired terminal manager menu and its styles; Reduce ContextValueDialog to the URL form it is mounted as; Remove retired Clean GUI plumbing from the renderer; Delete no-caller renderer wrappers and a dangling AgentCanvas doc; Delete dead and test-only conversation exports, props and pill rules; Delete renderer-side agent store mutators the Rust store replaced; Delete the removed attention pill's wording and other test-only views; Drop AppSelect's always-overridden injectStyles prop and test overloads; Stop sidebarRowLists computing fields its only caller ignores; Make Clear all clear local attention events when the harness is absent; Delete SessionLibrary's no-op hover-reveal rule; Hide the attention live region with the shared sr-only rule; Drop App's preload-skew guards and a duplicate workspace ref; Keep turn checkpoints in memory instead of per-run localStorage; Delete usage-menu CSS for markup the toolbar no longer renders; Drop the narrow toolbar spacing rule that never rendered; Delete CSS rules and glyphs for markup nothing renders; Drop token restatements that change nothing in two appearance blocks
- **[internal]** `73e41be` · 2026-09-23T08:20:57-05:00 · **Audit cleanup (services 1/5): dead code, dependency feature trims, blocking work off async workers** (`cleanup/audit-services-20260923`)
  - Notes: Install only the arm64 Rust target; Drop unused dependency features and the hex crate; Drop the async desktop dispatch wrapper and tokio dev-dependency; Delete unused desktop authority readers and restore DTO exports; Chmod iface private writes once, through the descriptor; Drop iface's never-compiled non-unix branches; Parse iface agent verbs straight into AgentMethod; Reuse ContextTelemetryCategory in iface doctor rows; Stop iface force exits from leaving an alternate screen inline; Stop iface creating an engine-history directory nothing reads; Trim interface-storage to the API its callers use; Keep the Rust invitation parser as a test oracle only; Cover the editor save-path race seams with ported oracle tests; Drop dead cfg(unix) gates in workspace, prompt-context and review; Drop ReviewCheckInvocation.shell and the ProcessOutput copy; Drop the zero-limit normalization in repository review; Drop unused serde derives from delegation ProposedFile; Drop xtask's never-compiled non-unix branches; Share one git is-ancestor helper across xtask; Stop Harness Context reading files after 20 suggestions; Skip the startup roster rewrite when no terminal name changes; Skip the reopen-authority sidecar write on title-only changes; Run prompt-context work on the blocking pool in the host; Cap iface's streaming redraws at one per 33 ms

### v0.3.130 — 2026-09-22T22:48:26-05:00 (`da45735`)

- **[feature]** `0c0a722` · 2026-09-22T22:47:45-05:00 · **Update packaged smoke for the Workspaces create menu** (`feat/new-project-from-workspaces-20260922`)
- **[feature]** `8902494` · 2026-09-22T21:52:23-05:00 · **Create projects from the Workspaces plus menu** (`feat/new-project-from-workspaces-20260922`)
  - Notes: Add project creation to the Workspaces plus menu

### v0.3.128 — 2026-09-22T18:17:00-05:00 (`07fa455`)

- **[feature]** `00ee105` · 2026-09-22T17:17:19-05:00 · **Hands-free app testing for agents** (`feat/handsfree-testing`)
  - Notes: Hands-free UI testing for agents; Keep hands-free sessions inside the session; Sync hands-free hook: silent default-menu block
- **[feature]** `de42bff` · 2026-09-22T16:51:05-05:00 · **Match reference menu transparency and charcoal tint** (`feat/translucent-popovers-20260922`)
  - Notes: Match the reference charcoal and visible menu translucency
- **[fix]** `07f528a` · 2026-09-22T15:50:42-05:00 · **Show Claude working on turns it starts itself** (`fix/hotkey-split-blank`)
- **[fix]** `73c9b15` · 2026-09-22T15:34:34-05:00 · **Keep Codex session lists refreshing on large histories** (`fix/hotkey-split-blank`)
- **[fix]** `312103a` · 2026-09-22T15:20:54-05:00 · **Record pane drag globe fix** (`fix/hotkey-split-blank`)
- **[fix]** `ca4a86e` · 2026-09-22T15:20:40-05:00 · **No globe icon while dragging a pane** (`fix/hotkey-split-blank`)
- **[fix]** `da1c88c` · 2026-09-22T15:12:24-05:00 · **Split grid panes in place** (`fix/hotkey-split-blank`)
- **[fix]** `f7748dc` · 2026-09-22T14:21:12-05:00 · **Record split hotkey follow-ups** (`fix/hotkey-split-blank`)
- **[fix]** `3a99b2a` · 2026-09-22T14:20:48-05:00 · **Blank terminal from the split hotkeys** (`fix/hotkey-split-blank`)
- **[feature]** `cc39ea4` · 2026-09-22T10:18:05-05:00 · **Subtle frosted menus and dialogs** (`feat/translucent-popovers-20260922`)
  - Notes: Add subtle frosted backgrounds to floating menus and dialogs
- **[record]** `fbb174c` · 2026-09-21T21:17:35-05:00 · **Record sidebar collections rollback verification** (`revert/sidebar-collections-20260921`)
- **[feature]** `0dacd08` · 2026-09-21T21:13:42-05:00 · **Restore the sidebar before the collections trial** (`revert/sidebar-collections-20260921`)
  - Notes: Withdraw the sidebar collections trial
- **[fix]** `cbc3520` · 2026-09-21T20:10:54-05:00 · **Record workspace deletion fix staged for production** (`fix/workspace-delete-20260921`)
  - Notes: Record approved 0.3.127 publication and staging
- **[fix]** `a92d867` · 2026-09-21T20:08:51-05:00 · **Integrate approved workspace deletion release** (`fix/workspace-delete-20260921`)
  - Notes: Fix workspace deletion error during terminal cleanup; chore(release): bump Interface to 0.3.127
- **[feature]** `a34a8bd` · 2026-09-21T19:47:53-05:00 · **Organize sidebar projects into collections** (`feat/sidebar-collections-20260921`)
  - Notes: Add persistent sidebar project collections
- **[fix]** `e8d0915` · 2026-09-21T19:30:25-05:00 · **Record workspace deletion release candidate** (`fix/workspace-delete-20260921`)
  - Notes: Record verified workspace deletion fix awaiting production approval

### v0.3.127 — 2026-09-21T19:23:29-05:00 (`d347d5b`)

- **[fix]** `f749dba` · 2026-09-21T19:22:52-05:00 · **Fix workspace deletion error during terminal cleanup** (`fix/workspace-delete-20260921`)
  - Notes: Fix workspace deletion racing terminal dismissal
- **[feature]** `69bc908` · 2026-09-21T19:09:11-05:00 · **Auto-tidy new terminal panes** (`codex/auto-tidy-new-panes-20260921`)
  - Notes: Auto-tidy the workspace when opening a new pane
- **[feature]** `7a0483e` · 2026-09-21T18:54:43-05:00 · **Create project folders from the app** (`feat/create-folders-in-picker-20260921`)
  - Notes: Enable creating project folders from the native picker
- **[record]** `fdd8934` · 2026-09-21T18:52:01-05:00 · **Record approved 0.3.126 updater staging** (`codex/release-smoke-settled-layout-20260921`)
  - Notes: Record approved 0.3.126 main promotion and updater staging
- **[record]** `085a41f` · 2026-09-21T18:49:29-05:00 · **Record verified 0.3.126 candidate awaiting scope approval** (`codex/release-smoke-settled-layout-20260921`)

### v0.3.126 — 2026-09-21T18:44:56-05:00 (`81b858a`)

- **[feature]** `8f18fa4` · 2026-09-21T18:44:31-05:00 · **Wait for terminal resize animation in packaged smoke** (`codex/release-smoke-settled-layout-20260921`)
  - Notes: Wait for settled pane geometry in packaged smoke
- **[fix]** `b0d29fe` · 2026-09-21T16:27:21-05:00 · **Start new panes in the workspace project, not the parent folder** (`fix/grok-pane-project-directory`)
  - Notes: Start new panes in the workspace project, not the parent folder.
- **[feature]** `c02f90c` · 2026-09-21T16:18:45-05:00 · **Record the Grok pane-name landing** (`feat/grok-pane-names`)
  - Notes: Record the Grok pane-name landing on Dev
- **[feature]** `0ad9db5` · 2026-09-21T16:17:56-05:00 · **Name Grok panes from the session title** (`feat/grok-pane-names`)
- **[fix]** `e0d5e9e` · 2026-09-21T15:00:41-05:00 · **Record verified 0.3.124 long Codex update staging** (`fix/codex-long-terminal-replay-20260919`)
  - Notes: docs: record long Codex fix production staging and verification
- **[fix]** `06fff32` · 2026-09-21T14:54:01-05:00 · **Verify retained long terminals in packaged releases** (`fix/codex-long-terminal-replay-20260919`)
  - Notes: test(release): verify long terminal preservation in packaged smoke
- **[fix]** `8f175c3` · 2026-09-21T14:43:15-05:00 · **Keep long Codex terminal fix compatible with production** (`fix/codex-long-terminal-replay-20260919`)
  - Notes: Keep retained terminal reattachment independent of pane animations

### v0.3.124 — 2026-09-21T14:38:59-05:00 (`4d864d6`)

- **[fix]** `94e633d` · 2026-09-19T21:41:44-05:00 · **Record long Codex terminal fix verification** (`fix/codex-long-terminal-replay-20260919`)
  - Notes: Record verified Dev landing for long Codex terminal fix
- **[fix]** `92407f5` · 2026-09-19T21:40:36-05:00 · **Preserve long Codex chats across workspace switches** (`fix/codex-long-terminal-replay-20260919`)
  - Notes: Preserve long-running terminal screens across workspace switches
- **[feature]** `ffa91d7` · 2026-09-19T11:15:32-05:00 · **Sleep idle workspace terminals and resume on selection** (`codex/workspace-terminal-sleep-20260919`)
  - Notes: Suspend idle terminals in inactive workspaces
- **[feature]** `2334417` · 2026-09-17T22:43:40-05:00 · **Anthropic Light and Anthropic Dark themes** (`feat/anthropic-theme`)
  - Notes: Record Anthropic themes in the to-do list; Extend three settled looks to the Anthropic palettes
- **[feature]** `e66b9c2` · 2026-09-17T20:10:43-05:00 · **Fluid sidebar slide with a fixed toggle** (`feat/sidebar-slide-20260917`)
- **[record]** `c405848` · 2026-09-17T12:01:41-05:00 · **Record 0.3.123 release of stuck renderer self-heal** (`docs/renderer-cpu-release-record-20260917`)
  - Notes: Record 0.3.123 release of the stuck renderer self-heal

### v0.3.123 — 2026-09-17T11:54:48-05:00 (`1136ef6`)

- **[speed]** `306ddec` · 2026-09-17T11:54:15-05:00 · **Release copy of stuck renderer self-heal for 0.3.123** (`perf/renderer-cpu-release-20260917`)
  - Notes: Release-compatible self-heal for a stuck renderer compositor
- **[speed]** `22f6b98` · 2026-09-17T11:43:57-05:00 · **Self-heal stuck renderer CPU and pause hidden windows** (`perf/renderer-cpu-20260917`)
  - Notes: Self-heal a stuck renderer compositor and truly pause hidden windows

### v0.3.121 — 2026-09-16T15:35:21-05:00 (`95525dc`)

- **[fix]** `77aec41` · 2026-09-16T15:35:12-05:00 · **Prepare standalone keyboard fix release** (`codex/release-keyboard-fix-20260916`)
  - Notes: Fix duplicated physical keyboard typing
- **[fix]** `6847376` · 2026-09-16T15:23:29-05:00 · **Fix duplicated physical keyboard typing** (`codex/fix-duplicate-physical-typing`)
  - Notes: Fix duplicated physical keyboard presses on macOS
- **[feature]** `903f5ad` · 2026-09-15T15:16:17-05:00 · **Smooth pane movement, maximize and swaps** (`feat/pane-motion-20260915`)
  - Notes: feat: animate terminal pane layout changes; docs: record the approved Dev pane motion trial
- **[feature]** `6b66ebf` · 2026-09-15T13:46:40-05:00 · **Record agent teammates landing in the to-do list** (`feat/agent-teammates-core-20260915`)
  - Notes: Record agent teammates landing on dev
- **[feature]** `16c183c` · 2026-09-15T13:45:00-05:00 · **Agent teammates: home folders, threads, memory and risky-act approvals** (`feat/agent-teammates-core-20260915`)
  - Notes: Plan agent teammates core implementation; Agent teammates contracts, approval facts hook and protocol 23; Engine approval gate, extra directories and approval facts; Agent store v2, home folders, decision log and desktop wiring; Agent teammate thread, memory, computer tabs and roster status; Agent approval policy and quote-aware shell tokenizer; Ask the owner before agents write harness configuration; Close approval gate bypasses found in review; Agent store fixes: idle close, import grants, safe temp files, grant roots; Close agent policy gaps: exec environment, git hooks and config, credential stores; Agent page: read-only past threads, focused sessions, inline refusals, latched restart notice; Close adversarial-review policy bypasses (fix round 2); Host agent threads, approval driver and runtime guard; Agent page: read a thread only when the thread itself is open; Close same-class policy variants (fix round 3); Agent sessions read-only outside their live thread or run; Resolve git paths from the -C base directory (fix round 4); Live agent gate probe harness and agent teammates docs; Close agent gate escapes found in final review; Unwrap Codex's harness shell wrapper before classifying; Model the directory changes the agent policy missed; Read git long-option abbreviations the way git does; Disable Codex's default-on acting features for gated agents; Keep Codex's code-mode host on for gated agents; Drop the && working-directory narrowing from the agent policy; Record agent teammates live probe results and current limits
- **[speed]** `13e23fa` · 2026-09-15T12:22:11-05:00 · **Record application review results and verified Dev landing** (`perf/application-review-20260915`)
  - Notes: Record completed application review and Dev verification
- **[speed]** `38d547a` · 2026-09-15T12:19:57-05:00 · **Improve search, transcript reads and idle resource use** (`perf/application-review-20260915`)
  - Notes: Reduce repeated search, history, polling and replay work
- **[record]** `4c862b1` · 2026-09-15T09:26:40-05:00 · **Record Away 0.3.119 staged update** (`docs/away-promotion-handoff-20260914`)
  - Notes: Mark Away 0.3.119 promotion and staging complete

### v0.3.119 — 2026-09-15T09:20:20-05:00 (`36d11b0`)

- **[record]** `2c72141` · 2026-09-15T08:44:14-05:00 · **Design for Grok Bot-style agent teammates (spec only)** (`docs/agent-teammates-core-spec-20260915`)
  - Notes: Design Grok Bot-style agent teammates core
- **[feature]** `0299a14` · 2026-09-14T21:34:47-05:00 · **New agent asks for name, purpose and icon first** (`feat/new-agent-identity-20260914`)
- **[feature]** `bfc57a8` · 2026-09-14T21:32:13-05:00 · **Record Grok 4.6 worker landing** (`feat/grok-worker`)
  - Notes: Record Grok 4.6 worker landing in TODO
- **[feature]** `908ec57` · 2026-09-14T21:31:24-05:00 · **Grok 4.6 available in the cheap_agents worker MCP** (`feat/grok-worker`)
  - Notes: Admit Grok 4.6 as a cheap_agents worker model
- **[fix]** `bd0606b` · 2026-09-14T21:20:45-05:00 · **Record completed performance review and 0.3.118 staging** (`fix/review-audit-v2-20260914`)
  - Notes: Record all review fixes staged in 0.3.118
- **[fix]** `86bd045` · 2026-09-14T21:17:22-05:00 · **Record completed audit fix and prepared release approval** (`fix/review-audit-v2-20260914`)
  - Notes: Record audit release verification and exact publication boundary

### v0.3.118 — 2026-09-14T21:11:54-05:00 (`7456e5c`)

- **[fix]** `f5f8c25` · 2026-09-14T21:10:43-05:00 · **Preserve complete audit history with bounded append cost** (`fix/review-audit-v2-20260914`)
  - Notes: Preserve complete audit history in lossless v2 segments
- **[fix]** `36b4792` · 2026-09-14T20:55:51-05:00 · **Record performance release preparation and approval boundaries** (`fix/review-performance-20260914`)
  - Notes: Record prepared performance release and pending approvals

### v0.3.117 — 2026-09-14T20:46:38-05:00 (`346da8d`)

- **[fix]** `f642ef1` · 2026-09-14T20:45:47-05:00 · **Bound parsing, history retention and worker capture** (`fix/review-performance-20260914`)
  - Notes: Bound parsing, runtime content retention and worker capture; Preserve the settled header fixture beside the focus regression
- **[record]** `d19c511` · 2026-09-14T18:49:43-05:00 · **Record Away promotion scope blocker** (`docs/away-promotion-handoff-20260914`)
  - Notes: Record blocked Away-only promotion and release scope
- **[internal]** `39e1cb9` · 2026-09-14T17:57:42-05:00 · **Retire and archive Clean GUI** (`chore/archive-clean-gui-20260914`)
  - Notes: Retire Clean GUI and preserve its source and design archive
- **[feature]** `af14596` · 2026-09-14T15:50:58-05:00 · **One-button project Away loops** (`feat/away-one-button-20260914`)
  - Notes: Add one-button project Away loops with native work and review
- **[fix]** `7882e96` · 2026-09-14T14:53:50-05:00 · **One attachment drop cue across terminal, Clean, chat and composer** (`fix/drop-indicator-everywhere-20260914`)
- **[record]** `ec02f85` · 2026-09-14T14:50:31-05:00 · **Track unfinished voice work and require resumable handoffs** (`docs/voice-control-handoff-20260914`)
  - Notes: Record unfinished voice work and require automatic task handoffs
- **[feature]** `1308161` · 2026-09-14T14:47:05-05:00 · **Compact menus follow the BridgeMind menu reference** (`feat/pane-menu-bridgemind-20260914`)
- **[fix]** `3608abd` · 2026-09-14T14:38:24-05:00 · **Provider icons for CLIs started from a shell pane** (`fix/detected-provider-icon`)
  - Notes: Show provider marks for CLIs started from a shell pane
- **[fix]** `46e743e` · 2026-09-14T13:56:27-05:00 · **Clean spacing aligns messages and composer** (`fix/clean-mode-recovery-20260914`)
  - Notes: Align Clean messages and composer on one reading column
- **[feature]** `22fac77` · 2026-09-14T12:52:22-05:00 · **Add control assistant rehearsal for desktop and iPhone** (`feat/voice-control-20260914`)
  - Notes: Add bounded control assistant rehearsal for desktop and iPhone
- **[fix]** `ed4ce86` · 2026-09-14T12:14:18-05:00 · **Menus that fit no longer scroll by half a pixel** (`fix/popover-fractional-height-20260914`)
- **[fix]** `8296384` · 2026-09-14T12:02:11-05:00 · **Clean Claude settings handle wrapped confirmations and titled prompts** (`fix/clean-mode-recovery-20260914`)
  - Notes: Recognize wrapped Claude effort receipts under titled prompts
- **[feature]** `3425e89` · 2026-09-14T11:54:01-05:00 · **Four-square button opens the workspace menu** (`feat/grid-menu-20260914`)
- **[fix]** `3755b2c` · 2026-09-14T11:49:54-05:00 · **Clean settings recover across Ultracode, redraws and request failures** (`fix/clean-mode-recovery-20260914`)
  - Notes: Fix Clean settings confirmation, uncertainty and recovery; Fence Clean settings while native yes/no approvals own input
- **[speed]** `b87f6a8` · 2026-09-14T11:04:27-05:00 · **Faster promote: optimised hashing, reused renderer build and icons** (`perf/promote-redundant-steps-20260914`)
- **[feature]** `6a1cd64` · 2026-09-14T11:01:20-05:00 · **Settings page follows the BridgeMind settings reference** (`feat/settings-reference-20260914`)
  - Notes: Keep the Pane appearance row line untouched

### v0.3.115 — 2026-09-14T10:37:16-05:00 (`ffddcb1`)

- **[fix]** `f8aa49f` · 2026-09-13T19:29:27-05:00 · **Pane Expand and New tab always visible** (`fix/pane-actions-always-visible`)
  - Notes: Keep pane Expand and New tab visible at rest
- **[fix]** `ab4ef88` · 2026-09-14T10:36:47-05:00 · **Repair Claude Clean detection and remove duplicate effort control** (`fix/clean-mode-recovery-20260914`)
  - Notes: Recognize native Claude in Clean and keep one settings menu
- **[fix]** `efc6e13` · 2026-09-14T10:31:05-05:00 · **OpenCode typed into a shell pane repaints after switching back** (`fix/shell-tui-replay-20260914`)
- **[feature]** `1f151cc` · 2026-09-14T10:25:15-05:00 · **Pane + opens the New pane menu; file drops show a path pill** (`feat/pane-menu-drop-20260914`)
- **[feature]** `676a139` · 2026-09-14T10:11:45-05:00 · **New panes share their row equally** (`feat/pane-row-insert-20260914`)
- **[fix]** `2783c49` · 2026-09-14T10:00:24-05:00 · **Restore reliable Clean model and effort controls** (`fix/clean-mode-recovery-20260914`)
  - Notes: Keep Clean settings changes working through terminal focus
- **[fix]** `0d626e3` · 2026-09-14T09:38:37-05:00 · **Repair Clean layout and restore working controls** (`fix/clean-mode-recovery-20260914`)
- **[feature]** `571ed22` · 2026-09-14T08:40:11-05:00 · **Clean/terminal: retry saved-pane reopen through the transient host-restart window** (`feat/clean-fresh-direction-b-20260914`)
  - Notes: Clean/terminal: retry saved-pane reopen through the transient restart window; docs(renderer): note the bounded reopen retry for saved panes
- **[feature]** `c513975` · 2026-09-14T08:22:19-05:00 · **Cheap Agents MCP name for Claude and Codex** (`feat/cheap-agents-name-20260914`)
  - Notes: Name the worker MCP cheap_agents for Claude and Codex
- **[feature]** `d00df76` · 2026-09-14T07:57:35-05:00 · **Clean: fresh Claude/Codex panes open on Direction B instead of the raw-CLI reader** (`feat/clean-fresh-direction-b-20260914`)
  - Notes: Clean: fresh Claude/Codex panes open on Direction B, not the raw CLI; docs(renderer): note the provisional Clean source for fresh panes
- **[feature]** `e651d84` · 2026-09-13T22:47:33-05:00 · **Reliable cheap workers in Claude and Codex panes** (`feat/worker-mcp-20260913`)
  - Notes: Make report workers reliable across native Claude and Codex panes; Fence older worker brokers before archived history is admitted
- **[feature]** `c9e5d8f` · 2026-09-13T22:32:12-05:00 · **Clean Direction B: approval card answers CLI prompts, in-pane Review changes with quote to prompt** (`feat/clean-direction-b-20260913`)
  - Notes: Clean approval card: answer CLI prompts in place; Clean review sheet: in-pane diff with quote to prompt; Clean approval card: the list the CLI printed, the key it printed; Clean approval card: the shortcut the dialog prints, and a mark that lifts; Renderer notes for the approval card and review sheet; Clean review sheet: the turn you pressed, the keys, the small pane
- **[feature]** `b77b22c` · 2026-09-13T21:55:57-05:00 · **Shared cheap report workers for Claude and Codex** (`feat/worker-mcp-20260913`)
  - Notes: Add opt-in shared report workers MCP for Claude and Codex
- **[feature]** `40020dc` · 2026-09-13T21:10:35-05:00 · **OpenCode sessions start with auto permissions** (`feat/opencode-auto-permissions-20260913`)
  - Notes: Launch OpenCode panes with auto permissions
- **[feature]** `10c084f` · 2026-09-13T21:00:49-05:00 · **Clean Direction B: panels, status pill, shimmer indicator, composer card, model menu, reveal strip** (`feat/clean-direction-b-20260913`)
  - Notes: Clean pane Direction B spec and plan; Clean composer card: one row until focused, model menu, Stop and Queue, reveal strip; Clean transcript: token palette, work panels, status pill, shimmer indicator; Clean composer review fixes: real copy, a name in the pill, slash returns itself; Clean review fixes: sink alignment, head order, declined edits; Clean Direction B: settled look and design notes; Hide the Clean effort pill below 430px, and keep the reveal rail on one line

### v0.3.114 — 2026-09-13T20:10:28-05:00 (`71a0299`)

- **[fix]** `2a2d94d` · 2026-09-13T19:37:57-05:00 · **OpenCode panes recover after busy workspace switches** (`fix/opencode-replay-20260913`)
  - Notes: Wait for OpenCode to process terminal redraw resizes
- **[fix]** `f8aa49f` · 2026-09-13T19:29:27-05:00 · **Pane Expand and New tab always visible** (`fix/pane-actions-always-visible`)
  - Notes: Keep pane Expand and New tab visible at rest
- **[feature]** `3fd69e6` · 2026-09-13T17:13:08-05:00 · **Clean review: approvals reveal, queued sends, mentions, reading polish, input setting** (`feat/clean-review-20260913`)
  - Notes: Clean input setting: composer or native CLI prompt; Clean reading polish: named folds, result phrase, neutral composer, strip and short-pane fixes; Name a lone Clean fold with its verb, and leave edits their diffstat; Clean composer: reveal approvals, hand off slash commands, queue while working; Reveal the CLI only when it is holding the keyboard; Veil the rail from the rail, and never say a fold's verb twice; Hold the Clean presentation while the emulator is scrolled back; Paint Clean's reading veil inside the scroller, under anything pinned; Clean composer @-mentions and Open changes on the turn chip; Clean review plan and renderer notes; Clean review fixes: findable payloads, honest folds and a pressable chip
- **[feature]** `60f3354` · 2026-09-13T14:24:54-05:00 · **Confirm extended-context models and finish Clean composer** (`feat/clean-composer-20260913`)
  - Notes: Confirm extended-context models and quiet routine draft status
- **[feature]** `0fd1297` · 2026-09-13T14:10:01-05:00 · **Custom Clean composer with model and effort selectors** (`feat/clean-composer-20260913`)
  - Notes: Use a custom Clean composer with model and effort selectors
- **[feature]** `d6dc6f4` · 2026-09-13T13:43:07-05:00 · **Color and depth for Clean conversations** (`feat/clean-color-20260913`)
  - Notes: Add color and depth to Clean conversations
- **[feature]** `3c87e9f` · 2026-09-13T12:44:22-05:00 · **Cleaner reading inspired by T3 Code and MonoCode** (`feat/clean-reading-20260913`)
  - Notes: Simplify Clean reading with quieter chrome and folded activity
- **[feature]** `85a151d` · 2026-09-13T12:15:05-05:00 · **Clean pane guidance and stable disclosure verification** (`feat/clean-native-panes-20260913`)
  - Notes: Clarify Clean native input and await rendered tool details
- **[feature]** `d37bcdc` · 2026-09-13T12:04:58-05:00 · **Clean conversations with native CLI input** (`feat/clean-native-panes-20260913`)
  - Notes: Present clean conversations above native CLI input

### v0.3.113 — 2026-09-13T08:57:45-05:00 (`141cbad`)

- **[feature]** `a9df308` · 2026-09-12T22:23:08-05:00 · **Harness commands, task evidence, shared drafts and bounded pairing** (`feat/harness-usability-20260912`)
  - Notes: Add harness commands, durable task evidence and shared drafts

### v0.3.112 — 2026-09-12T14:20:49-05:00 (`20f01d1`)

- **[feature]** `afe6471` · 2026-09-12T14:19:09-05:00 · **Packaged smoke pins the four-column launcher roster** (`feat/aesthetic-review-20260912`)
  - Notes: Packaged smoke pins the launcher's four-column roster
- **[fix]** `e05c485` · 2026-09-12T13:47:54-05:00 · **Text cursor over selectable terminal text** (`fix/terminal-text-cursor-20260912`)
- **[feature]** `54277c1` · 2026-09-12T13:39:36-05:00 · **Quiet New-tab trigger on pane heads** (`feat/aesthetic-review-20260912`)
  - Notes: Quiet the New-tab menu trigger too
- **[feature]** `c5f6470` · 2026-09-12T13:34:34-05:00 · **Aesthetic review: quieter chrome, one label idiom, one Send, product mark** (`feat/aesthetic-review-20260912`)
  - Notes: Reveal quiet pane actions through :is(), leaving the hover-tint marker untouched
- **[internal]** `eae231e` · 2026-09-12T13:26:11-05:00 · **Efficiency cleanup: push batching, uuid helper, orchestrator retirement, App.tsx hooks** (`cleanup/efficiency-20260912`)
  - Notes: Ignore iCloud conflict copies and ship an optimised release profile; One canonical UUID check in interface-common; Flush queued host pushes together; Retire the unimplemented orchestrator host from the collaboration broker; Lift six self-contained state groups out of App.tsx into hooks
- **[fix]** `3fd566d` · 2026-09-12T12:53:44-05:00 · **Pane-close visibility guard in the collapse fixture** (`fix/pane-close-slot-check`)
  - Notes: Assert survivors stay visible after each pane close
- **[feature]** `9f4e2e6` · 2026-09-12T12:32:40-05:00 · **Run reviewed Away cycles from Codex and Claude Code** (`feat/away-command-review-20260912`)
  - Notes: Add reviewed Away cycles for Codex and Claude Code
- **[feature]** `267e430` · 2026-09-12T11:44:49-05:00 · **Reject malformed Away baseline evidence** (`feat/away-validation-evidence-20260912`)
- **[feature]** `137538e` · 2026-09-12T11:41:37-05:00 · **Record Away baseline evidence landing** (`feat/away-validation-evidence-20260912`)
- **[feature]** `305255a` · 2026-09-12T11:40:33-05:00 · **Retain failed Away baseline evidence** (`feat/away-validation-evidence-20260912`)
- **[feature]** `9a83cc7` · 2026-09-12T11:37:08-05:00 · **Record reviewed Away correction landing** (`feat/away-validation-evidence-20260912`)
- **[feature]** `98f65e8` · 2026-09-12T11:36:07-05:00 · **Compare Away candidate changes for no progress** (`feat/away-validation-evidence-20260912`)
- **[feature]** `ca8b908` · 2026-09-12T11:29:57-05:00 · **Close Away cycle at deadline** (`feat/away-validation-evidence-20260912`)
- **[feature]** `cdea1cc` · 2026-09-12T11:15:18-05:00 · **Record Away vault handoff blocker** (`feat/away-validation-evidence-20260912`)
- **[feature]** `7498eea` · 2026-09-12T11:11:47-05:00 · **Record Away cycle audit and runtime evidence** (`feat/away-validation-evidence-20260912`)
- **[feature]** `defdd49` · 2026-09-12T11:09:58-05:00 · **Retain Away validation infrastructure failures** (`feat/away-validation-evidence-20260912`)
- **[feature]** `d5ca20c` · 2026-09-12T11:05:16-05:00 · **Retain Away baseline evidence before dispatch** (`feat/away-baseline-20260912`)
- **[feature]** `234da0d` · 2026-09-12T11:00:33-05:00 · **Expose unknown Away outcomes to status callers** (`feat/away-status-20260912`)
- **[feature]** `675d2aa` · 2026-09-12T10:54:01-05:00 · **Honor Away Stop during provider waits** (`feat/away-stop-20260912`)
- **[feature]** `fcd46e5` · 2026-09-12T10:48:10-05:00 · **Retain Away validation context across retries** (`feat/away-feedback-20260912`)
- **[feature]** `d2250d0` · 2026-09-12T10:41:38-05:00 · **Prevent Away from spending retries on duplicate failed proposals** (`feat/away-hour-20260912`)
  - Notes: Stop Away retries after repeated proposals
- **[fix]** `88c9c5a` · 2026-09-12T10:13:04-05:00 · **Pair iPhone when local discovery is unavailable** (`fix/phone-direct-address-20260912`)
  - Notes: Fix iPhone pairing when Bonjour discovery is unavailable
- **[record]** `986f891` · 2026-09-12T10:01:25-05:00 · **Checked agent navigation and bounded-task guide** (`docs/agent-navigation-20260912`)
  - Notes: Add checked task navigation and agent audit; Record navigation audit validation evidence

### v0.3.109 — 2026-09-12T09:16:05-05:00 (`7865ad8`)

- **[fix]** `78a73dd` · 2026-09-11T23:06:38-05:00 · **Click to place the cursor in unshaded Codex prompts** (`fix/terminal-click-cursor-20260911`)
  - Notes: Recognize the unshaded native Codex prompt layout
- **[feature]** `1046a3d` · 2026-09-11T23:03:23-05:00 · **Bounded Away pilot with validated repairs and Stop** (`feat/away-pilot-20260911`)
  - Notes: Add bounded Away pilot with durable validation and cancellation
- **[feature]** `2c2f92b` · 2026-09-11T22:59:47-05:00 · **Pin projects alongside workspaces** (`feat/project-pinning-20260911`)
  - Notes: Add persistent project pins alongside workspace pins
- **[fix]** `49f802e` · 2026-09-11T22:56:29-05:00 · **Fix black panes when opening or switching workspaces** (`fix/blank-new-panes-20260911`)
  - Notes: Remove retired pane mounts when switching workspaces
- **[fix]** `8137568` · 2026-09-11T22:52:46-05:00 · **Click to place the cursor in terminal prompts** (`fix/terminal-click-cursor-20260911`)
  - Notes: Add click-to-place cursor in native terminal prompts

### v0.3.107 — 2026-09-11T22:19:52-05:00 (`46f8cd5`)

- **[feature]** `aa46d6f` · 2026-09-11T21:00:29-05:00 · **Encrypted iPhone QR pairing and direct connection** (`feat/iphone-direct-connect-20260911`)
  - Notes: feat(remote): add encrypted QR pairing and native phone transport

### v0.3.106 — 2026-09-11T18:25:55-05:00 (`dff160e`)

- **[feature]** `3564df8` · 2026-09-11T18:25:06-05:00 · **Redesign iPhone sessions, conversations and approvals** (`feat/iphone-design-20260911`)
  - Notes: Redesign the iPhone library and conversation experience
- **[fix]** `00645e7` · 2026-09-11T17:59:55-05:00 · **Stop iPhone Face ID retry loops and recover Mac pairing** (`fix/iphone-faceid-loop-20260911`)
  - Notes: Prevent repeated iPhone Face ID prompts after refused pairing

### v0.3.105 — 2026-09-11T17:26:35-05:00 (`7550809`)

- **[feature]** `af32d1f` · 2026-09-11T14:53:12-05:00 · **Trusted iPhone pairing with Face ID and native workspaces** (`feat/iphone-trusted-access-20260911`)
  - Notes: Add trusted iPhone pairing and native Face ID workspace navigation
- **[feature]** `17607a1` · 2026-09-11T12:45:35-05:00 · **Native iPhone companion with shared Interface sessions** (`feat/iphone-app-20260911`)
  - Notes: Add native iPhone companion with shared workspace and conversation views; Handle unavailable Mac gateways in the iPhone shell
- **[feature]** `eeedf7b` · 2026-09-11T09:36:07-05:00 · **iPhone remote companion for terminal sessions** (`feat/iphone-remote-20260911`)
  - Notes: Add iPhone remote companion for native terminal sessions
- **[feature]** `e8da966` · 2026-09-11T17:25:55-05:00 · **Interface iPhone app icon and production connection guide** (`feat/iphone-brand-production-20260911`)
  - Notes: Add Interface Fold branding to the iPhone app
- **[feature]** `963bb47` · 2026-09-11T17:04:27-05:00 · **Refined iPhone conversations with replies and approvals** (`feat/iphone-conversation-20260911`)
  - Notes: Refined iPhone conversations with replies and single-use approvals

### v0.3.104 — 2026-09-11T16:03:59-05:00 (`70def0e`)

- **[fix]** `c4d01b7` · 2026-09-11T16:02:26-05:00 · **Prevent terminal animation freezes** (`fix/terminal-announcement-lag-20260911`)
  - Notes: Bound terminal announcements to prevent animation layout freezes
- **[feature]** `af32d1f` · 2026-09-11T14:53:12-05:00 · **Trusted iPhone pairing with Face ID and native workspaces** (`feat/iphone-trusted-access-20260911`)
  - Notes: Add trusted iPhone pairing and native Face ID workspace navigation
- **[feature]** `17607a1` · 2026-09-11T12:45:35-05:00 · **Native iPhone companion with shared Interface sessions** (`feat/iphone-app-20260911`)
  - Notes: Add native iPhone companion with shared workspace and conversation views; Handle unavailable Mac gateways in the iPhone shell
- **[feature]** `53468f3` · 2026-09-11T12:36:39-05:00 · **Show a floating Swap preview when dragging terminal panes** (`feat/pane-swap-preview-20260911`)
  - Notes: Add a floating terminal pane swap preview

### v0.3.103 — 2026-09-11T11:02:44-05:00 (`6ef0d56`)

- **[speed]** `77cb108` · 2026-09-11T00:22:22-05:00 · **Faster terminal output and lighter idle work** (`perf/responsiveness-20260910`)
  - Notes: Avoid quadratic terminal scans and redundant idle updates
- **[fix]** `b2f3690` · 2026-09-11T09:40:40-05:00 · **Stable Codex pane and workspace names** (`fix/stable-codex-pane-names-20260911`)
  - Notes: Keep Codex pane and workspace task names stable
- **[feature]** `eeedf7b` · 2026-09-11T09:36:07-05:00 · **iPhone remote companion for terminal sessions** (`feat/iphone-remote-20260911`)
  - Notes: Add iPhone remote companion for native terminal sessions
- **[speed]** `0368903` · 2026-09-11T08:40:03-05:00 · **Clarify terminal interaction validation** (`perf/terminal-interaction-20260911`)
  - Notes: Clarify the interaction follow-up scoped validation
- **[speed]** `7f1dbe6` · 2026-09-11T08:37:33-05:00 · **Lighter terminal focus and pane switching** (`perf/terminal-interaction-20260911`)
  - Notes: Avoid redundant terminal focus renders and geometry reads

### v0.3.102 — 2026-09-11T08:35:32-05:00 (`0b996a3`)

- **[speed]** `77cb108` · 2026-09-11T00:22:22-05:00 · **Faster terminal output and lighter idle work** (`perf/responsiveness-20260910`)
  - Notes: Avoid quadratic terminal scans and redundant idle updates
- **[fix]** `af80b28` · 2026-09-10T23:49:39-05:00 · **Restore full-width terminal input spacing** (`fix/restore-terminal-edge-spacing-20260910`)
  - Notes: Restore full-width native terminal input spacing

### v0.3.101 — 2026-09-10T17:14:31-05:00 (`3beeada`)

- **[fix]** `da15c36` · 2026-09-10T17:14:10-05:00 · **Align packaged smoke with compact workspace layout** (`fix/compact-layout-package-smoke-20260910`)
- **[fix]** `d941f35` · 2026-09-10T16:54:52-05:00 · **Restore compact Interface scale with reference layout and colors** (`fix/compact-reference-layout-20260910`)
- **[fix]** `0bf3e72` · 2026-09-10T16:41:57-05:00 · **Reduce terminal text to 18px** (`fix/terminal-reading-scale-18px-20260910`)
  - Notes: Reduce default terminal reading size to 18px
- **[feature]** `baf4b65` · 2026-09-10T16:11:14-05:00 · **Match BridgeMind workspace scale and navigation** (`feat/reference-workspace-scale-20260910`)
  - Notes: Match the reference workspace scale and navigation

### v0.3.99 — 2026-09-10T14:57:38-05:00 (`2a71cd7`)

- **[fix]** `8b407c6` · 2026-09-10T14:56:59-05:00 · **Align packaged smoke checks with the approved workspace layout** (`fix/release-layout-smoke-20260910`)
- **[feature]** `ceddfaa` · 2026-09-10T11:04:39-05:00 · **Workspace launcher with provider tiles and launch preview** (`feat/workspace-launcher-reference-20260910`)
  - Notes: Match workspace launcher to the provider-grid reference
- **[fix]** `68f6930` · 2026-09-10T10:52:51-05:00 · **Make terminal scrollbars thin and subdued** (`fix/thin-terminal-scrollbars-20260910`)

### v0.3.97 — 2026-09-10T10:23:44-05:00 (`de0f74f`)

- **[fix]** `b1f7830` · 2026-09-10T10:23:03-05:00 · **Prepare the standalone pane header effort removal** (`fix/production-header-effort-20260910`)
  - Notes: Remove the effort selector from terminal pane headers

### v0.3.96 — 2026-09-10T08:50:18-05:00 (`9ea5b74`)

- **[feature]** `b58ea4d` · 2026-09-10T08:49:37-05:00 · **Validate terminal padding through late CLI resize redraws** (`release/terminal-side-padding-20260910`)
  - Notes: Remove native terminal side padding
- **[fix]** `923f81b` · 2026-09-10T08:24:13-05:00 · **Keep terminal spacing current when reopening Dev** (`fix/dev-dock-terminal-spacing-20260910`)
  - Notes: Refresh the built renderer before launching Dev
- **[fix]** `55d6645` · 2026-09-09T23:10:34-05:00 · **Expand native terminal entry bars to the pane edges** (`fix/terminal-edge-spacing-20260909`)
  - Notes: Expand native terminal content to the pane edges; Record native Clean controls using the full pane width
- **[feature]** `6ca89da` · 2026-09-09T23:03:38-05:00 · **Give workspace names and reading controls more room** (`feat/readable-workspace-20260909`)
- **[fix]** `cef5eca` · 2026-09-09T21:02:21-05:00 · **Remove effort selector from pane headers** (`fix/remove-header-effort-20260909`)
- **[feature]** `2edcd2b` · 2026-09-09T20:20:25-05:00 · **Readable workspace hierarchy and task titles** (`feat/workspace-hierarchy-20260909`)
  - Notes: Refine workspace hierarchy, readability, and automatic task titles

### v0.3.94 — 2026-09-09T18:45:20-05:00 (`70a0c6e`)

- **[fix]** `32e0ed6` · 2026-09-09T18:41:25-05:00 · **Compact header controls and readable notification counts** (`fix/compact-workspace-header-20260909`)
  - Notes: Refine workspace header proportions and notification count

### v0.3.93 — 2026-09-09T18:18:10-05:00 (`07529ab`)

- **[feature]** `07341f8` · 2026-09-09T17:04:08-05:00 · **Native pane tools retain access after workspace reopen** (`feat/pane-menu-actions-20260909`)
  - Notes: Retain native pane tool access after saved workspace reopen
- **[feature]** `28dc662` · 2026-09-09T16:53:08-05:00 · **Canvas follows folders opened by terminal panes** (`feat/pane-menu-actions-20260909`)
  - Notes: Keep terminal-only project folders in the Canvas picker
- **[feature]** `2937846` · 2026-09-09T16:47:48-05:00 · **Workspace tools and existing agent launches in the pane menu** (`feat/pane-menu-actions-20260909`)
  - Notes: Add workspace tools and existing launchers to the pane menu

### v0.3.92 — 2026-09-09T16:11:46-05:00 (`687b11c`)

- **[fix]** `0fed591` · 2026-09-09T16:10:32-05:00 · **Settings below Workspaces** (`fix/sidebar-settings-20260909`)
  - Notes: Move Settings below Workspaces in the sidebar; Record the approved sidebar Settings footer

### v0.3.91 — 2026-09-09T15:13:02-05:00 (`dd2f5eb`)

- **[fix]** `ba895e2` · 2026-09-09T15:08:19-05:00 · **Effort selector matches pane header** (`fix/effort-header-style-20260909`)
  - Notes: Match effort selector to pane header controls

### v0.3.90 — 2026-09-09T13:52:23-05:00 (`e220a48`)

- **[fix]** `e961a7b` · 2026-09-09T11:55:36-05:00 · **Compact centered Agent Code Chat header** (`fix/topbar-reference-20260909`)
  - Notes: Match the window header to the compact navigation reference

### v0.3.89 — 2026-09-09T10:34:40-05:00 (`962c4fc`)

- **[fix]** `393d9d9` · 2026-09-09T10:29:44-05:00 · **Visible idle pane borders and tighter bottom spacing** (`fix/pane-edges-footer-20260909`)
  - Notes: Keep idle pane borders visible and tighten bottom spacing

### v0.3.88 — 2026-09-09T10:06:08-05:00 (`b482af1`)

- **[speed]** `5820280` · 2026-09-09T09:36:55-05:00 · **Faster transcripts and bounded background work** (`perf/review-fixes-20260909`)
  - Notes: perf: bound presentation work and simplify controller ownership
- **[feature]** `f7b21fe` · 2026-09-09T09:04:41-05:00 · **Durable agents and local scheduled routines** (`feat/agent-foundation-20260909`)
  - Notes: Build durable agents and local scheduled routines
- **[fix]** `b3b3b6d` · 2026-09-09T08:39:05-05:00 · **Two-by-three workspace activity ticker** (`fix/sidebar-ticker-2x3-20260909`)
  - Notes: fix(sidebar): use a two-by-three workspace ticker
- **[feature]** `a15bb2a` · 2026-09-09T08:30:42-05:00 · **Sidebar reference styling and active workspace ticker** (`feat/sidebar-reference-20260909`)
  - Notes: Match the sidebar reference and tick active CLI workspaces

### v0.3.87 — 2026-09-09T08:28:38-05:00 (`7389144`)

- **[feature]** `27f9212` · 2026-09-09T08:25:53-05:00 · **Keep top-bar effort menus open when changing panes** (`feat/topbar-effort-20260909`)
  - Notes: Keep effort menus focused when activating another pane
- **[feature]** `1aba3c5` · 2026-09-09T08:11:45-05:00 · **Agent effort selector in terminal pane top bars** (`feat/topbar-effort-20260909`)
  - Notes: Add agent effort selection to terminal pane headers
- **[fix]** `8f0eaf9` · 2026-09-09T08:05:44-05:00 · **Reserve the Code sidebar for CLI workspaces** (`fix/cli-only-code-20260909`)
  - Notes: Keep non-CLI conversations out of the Code sidebar
- **[fix]** `738e7ab` · 2026-09-09T07:44:40-05:00 · **Correct Chat and Code conversation categories** (`fix/chat-code-categories-20260909`)
  - Notes: Keep uncategorized conversations under Chat

### v0.3.86 — 2026-09-09T07:32:28-05:00 (`4b5257a`)

- **[fix]** `5c060e6` · 2026-09-09T07:31:47-05:00 · **Disable the production delegation layer** (`fix/disable-production-delegation-20260909`)
  - Notes: Disable the production delegation workflow while retaining its implementation
- **[fix]** `f9314d1` · 2026-09-08T23:20:54-05:00 · **Grey selected pane headers; purple on hover** (`fix/selected-pane-grey-20260908`)
  - Notes: Restore grey selected pane headers
- **[fix]** `0775ae3` · 2026-09-08T23:14:53-05:00 · **Restore dots in the sidebar and pane headers** (`fix/clean-sidebar-dot-20260909`)
  - Notes: Restore sidebar and pane-header working dots

### v0.3.85 — 2026-09-08T22:29:56-05:00 (`14ecd21`)

- **[feature]** `65769be` · 2026-09-08T20:50:21-05:00 · **Model and effort controls in Clean terminal bottom bars** (`feat/clean-bottom-controls-20260908`)
  - Notes: Add model and effort controls to Clean terminal composers; Preserve settings and attachment errors in Clean composers
- **[feature]** `eeefdca` · 2026-09-08T20:39:49-05:00 · **Wave working indicators throughout Clean mode** (`feat/clean-working-wave-20260909`)
  - Notes: Use the Wave working indicator throughout Clean mode; Record the approved Wave in Clean appearance decisions
- **[fix]** `a7eb05b` · 2026-09-08T20:24:45-05:00 · **Close finished delegated worker panes automatically** (`fix/delegate-pane-auto-close-20260908`)
  - Notes: Close delegated worker panes when their work finishes
- **[fix]** `3e7f946` · 2026-09-08T20:18:05-05:00 · **Image attachments in Clean terminal conversations** (`fix/clean-image-attachments-20260908`)
  - Notes: Fix image attachments in Clean terminal conversations

### v0.3.84 — 2026-09-08T19:43:28-05:00 (`4d5f9ae`)

- **[fix]** `4061cde` · 2026-09-08T19:30:35-05:00 · **Visible Clean composer actions in dark themes** (`fix/clean-conversation-panes-20260908`)
  - Notes: Fix Clean composer action contrast in dark themes
- **[fix]** `70d3dc6` · 2026-09-08T19:21:42-05:00 · **Clean terminal conversations with chat input** (`fix/clean-conversation-panes-20260908`)
  - Notes: Render Clean terminal panes as conversations
- **[fix]** `60f7544` · 2026-09-08T18:53:25-05:00 · **Swap pane positions when dragging their headers** (`fix/pane-header-swap-20260908`)
  - Notes: Swap whole terminal panes on header drop

### v0.3.83 — 2026-09-08T18:53:10-05:00 (`e7130ae`)

- **[fix]** `dcfc575` · 2026-09-08T18:17:25-05:00 · **Show native OpenCode workers and prefer GLM or DeepSeek** (`fix/delegate-native-session-20260908`)
- **[feature]** `e6ffdb2` · 2026-09-08T18:07:31-05:00 · **Read CLI output in Clean panes** (`feat/clean-cli-reader-20260908`)
  - Notes: Render CLI output as readable Clean panes

### v0.3.81 — 2026-09-08T17:50:38-05:00 (`c870e34`)

- **[fix]** `2a6b89d` · 2026-09-08T17:42:58-05:00 · **Recover from blocked delegation handoffs** (`fix/delegation-handoff-recovery-20260908`)
  - Notes: Fix recovery after a blocked delegation handoff

### v0.3.80 — 2026-09-08T15:49:09-05:00 (`c0183dd`)

- **[fix]** `9433310` · 2026-09-08T15:40:46-05:00 · **Purple header hover without pane outline** (`fix/pane-header-purple-hover-20260908`)
  - Notes: Limit pane hover to a purple header tint

### v0.3.79 — 2026-09-08T15:10:43-05:00 (`9bc963f`)

- **[feature]** `da212f0` · 2026-09-08T15:07:45-05:00 · **Pane header hover glow** (`feat/pane-header-hover-20260908`)
  - Notes: Add soft pane header hover highlight

### v0.3.78 — 2026-09-08T15:07:10-05:00 (`af3c7f9`)

- **[feature]** `1b0f47f` · 2026-09-08T15:04:49-05:00 · **Check the folder-scoped Agent composer in packaged smoke** (`feat/unified-pane-appearance-20260908`)
  - Notes: Check folder-scoped Agent composer in packaged smoke
- **[feature]** `bd4eb77` · 2026-09-08T14:59:52-05:00 · **Delegate between Claude and Codex alongside OpenCode and Antigravity** (`feat/claude-codex-delegation-20260908`)
  - Notes: Support Claude and Codex as bounded delegation workers
- **[fix]** `563f980` · 2026-09-08T14:56:35-05:00 · **Restore terminal launches in saved workspaces after restart** (`fix/terminal-approved-root-20260908`)
  - Notes: Fix fresh terminal launches in saved workspace roots after restart
- **[feature]** `7dd2040` · 2026-09-08T12:46:19-05:00 · **Check Chat composer independently of Code startup** (`feat/unified-pane-appearance-20260908`)
  - Notes: Open Chat explicitly for packaged composer checks
- **[feature]** `7c60ad1` · 2026-09-08T12:38:15-05:00 · **Verify unified workspace creation in packaged smoke** (`feat/unified-pane-appearance-20260908`)
  - Notes: Keep packaged workspace diagnostics aligned with unified creation
- **[feature]** `d0b402e` · 2026-09-08T12:25:27-05:00 · **Open Code at its CLI workspace on startup** (`feat/unified-pane-appearance-20260908`)
- **[feature]** `1d928ee` · 2026-09-08T12:16:57-05:00 · **Use one CLI workspace with Terminal or Clean appearance** (`feat/unified-pane-appearance-20260908`)
  - Notes: Make Clean an appearance setting for the existing CLI panes
- **[feature]** `fd3c652` · 2026-09-08T12:09:37-05:00 · **Delegate tasks to Antigravity alongside OpenCode** (`feat/agy-delegation-20260908`)
- **[feature]** `bceea0d` · 2026-09-08T11:37:59-05:00 · **Show delegated workers beside their principal** (`codex/visible-delegation-split-20260908`)
  - Notes: feat(terminal): show delegated workers beside the principal

### v0.3.74 — 2026-09-08T09:22:22-05:00 (`5392f5f`)

- **[fix]** `6fe038a` · 2026-09-08T09:21:51-05:00 · **Verify terminal workspace creation through the type picker** (`fix/workspace-picker-smoke-20260908`)
  - Notes: fix(release): select terminal type in packaged workspace smoke
- **[feature]** `634dfd0` · 2026-09-08T07:47:13-05:00 · **Clear dismissed content behind glass panes** (`codex/glass-rendering-fixes-20260908`)
  - Notes: Create the native glass compositor before the first paint
- **[fix]** `a5afdfc` · 2026-09-08T07:40:42-05:00 · **Fix glass rendering and pane fade-ins** (`codex/glass-rendering-fixes-20260908`)
  - Notes: Fix glass rendering and visible-window repainting
- **[feature]** `cb57308` · 2026-09-07T23:16:47-05:00 · **Create Clean or Terminal workspaces with type icons** (`feat/clean-terminal-workspaces-20260908`)
  - Notes: feat: create Clean or Terminal workspaces from one workspace action
- **[feature]** `8ead6ba` · 2026-09-07T22:53:51-05:00 · **Give translucency controls room in Appearance** (`codex/pane-translucency-20260907`)
  - Notes: Give translucency sliders room in Appearance settings
- **[feature]** `cad2608` · 2026-09-07T22:40:40-05:00 · **Adjust terminal and sidebar translucency** (`codex/pane-translucency-20260907`)
  - Notes: Add independent terminal and sidebar translucency controls
- **[feature]** `eb680b7` · 2026-09-07T22:17:06-05:00 · **Optional clean Code panes with inline agent activity** (`feat/clean-conversation-panes-20260908`)
  - Notes: Add optional clean Code panes and inline agent activity
- **[feature]** `79b9d34` · 2026-09-07T19:25:03-05:00 · **Compare seven worker models on real repository tasks** (`codex/delegation-model-comparison-20260907`)
  - Notes: Compare seven delegation models with real repository evidence

### v0.3.72 — 2026-09-07T18:48:05-05:00 (`976d30c`)

- **[feature]** `fa46cce` · 2026-09-07T18:17:40-05:00 · **Keep worker quality evidence consistent after explicit review** (`codex/delegation-worker-quality-20260907`)
- **[feature]** `457eecc` · 2026-09-07T17:59:52-05:00 · **Validate delegated proposals before acceptance without worker budgets** (`codex/delegation-worker-quality-20260907`)
  - Notes: Validate delegated proposals and remove fixed worker budgets
- **[internal]** `3d24074` · 2026-09-07T16:56:48-05:00 · **Evaluate real Codex and OpenCode on a cloned School OS task** (`codex/delegation-real-repo-20260907`)
  - Notes: test(delegation): evaluate real Codex work in a cloned repository
- **[record]** `b5f6d35` · 2026-09-07T16:28:49-05:00 · **Record delegation evaluation and package verification** (`codex/delegation-workflow-20260907`)
  - Notes: Record delegation release checks and current bridge contract
- **[feature]** `dd4a6ad` · 2026-09-07T16:23:49-05:00 · **Review delegated work before integration and show worker status** (`codex/delegation-workflow-20260907`)
  - Notes: Add pane-owned delegation decisions, review, and quality checks
- **[fix]** `6d0da33` · 2026-09-07T16:09:48-05:00 · **Every theme follows the Dark workspace design** (`fix/consistent-themes-20260907`)
  - Notes: feat(renderer): unify themes with the Dark workspace design
- **[fix]** `fcb91cc` · 2026-09-07T15:31:27-05:00 · **The last terminal fills its workspace after sibling panes close** (`fix/terminal-single-pane-20260907`)
  - Notes: Keep the last terminal pane full-sized after closing its siblings
- **[feature]** `54632d1` · 2026-09-07T15:22:05-05:00 · **Track OpenCode Go allowance in the Usage menu** (`feat/opencode-go-usage-20260907`)
  - Notes: Track OpenCode Go subscription usage and reset windows

### v0.3.71 — 2026-09-07T13:15:38-05:00 (`eeec02e`)

- **[fix]** `e2277e0` · 2026-09-07T13:12:43-05:00 · **OpenCode panes retain selected chats and recover missing resume IDs** (`fix/opencode-resume-recovery-20260907`)
  - Notes: Fix OpenCode selected session capture and restart recovery

### v0.3.70 — 2026-09-07T12:37:53-05:00 (`43bcf29`)

- **[fix]** `e89d062` · 2026-09-07T12:30:05-05:00 · **OpenCode spend in the usage panel** (`fix/opencode-pane-logo-20260907`)
  - Notes: feat(usage): report OpenCode token spend in the usage panel
- **[feature]** `421748a` · 2026-09-07T12:18:35-05:00 · **OpenCode panes resume their chat across restarts** (`feat/opencode-resume-20260907`)
- **[fix]** `4a271aa` · 2026-09-07T12:03:47-05:00 · **OpenCode panes show working and waiting activity** (`codex/opencode-activity-20260907`)
  - Notes: fix(terminal): track OpenCode activity from native TUI state
- **[fix]** `2477407` · 2026-09-07T11:49:08-05:00 · **OpenCode panes show their own logo** (`fix/opencode-pane-logo-20260907`)
  - Notes: fix(renderer): show the official OpenCode pane logo
- **[fix]** `6fa1a96` · 2026-09-07T11:47:00-05:00 · **OpenCode history keeps scrolling after workspace switches** (`fix/opencode-scrollback-20260907`)
  - Notes: Preserve OpenCode scrolling modes when panes remount
- **[fix]** `beb0d0c` · 2026-09-07T11:33:34-05:00 · **OpenCode panes redraw completely after workspace switches** (`fix/opencode-pane-render-20260907`)
  - Notes: Keep terminal redraw pulses beyond OpenCode resize debounce
- **[feature]** `c0f0925` · 2026-09-07T10:51:54-05:00 · **Agents delegate background tasks to OpenCode Go** (`feat/delegate-workers-20260907`)
  - Notes: Delegate bounded terminal tasks to OpenCode Go workers

### v0.3.68 — 2026-09-07T09:57:50-05:00 (`44feef2`)

- **[fix]** `443186f` · 2026-09-07T09:57:25-05:00 · **Release smoke verifies every terminal provider** (`codex/package-roster-smoke-20260907`)
  - Notes: fix(release): verify the complete terminal provider roster
- **[feature]** `e636b54` · 2026-09-07T09:32:29-05:00 · **Keep the terminal Fast bolt clear at small sizes** (`codex/terminal-fast-toggle-20260907`)
  - Notes: Keep the terminal Fast bolt legible at header size
- **[feature]** `7e353ab` · 2026-09-07T09:27:39-05:00 · **OpenCode is available in the terminal provider picker** (`feat/opencode-terminal`)
  - Notes: Add OpenCode to integrated terminal profiles
- **[feature]** `514ef0d` · 2026-09-07T09:23:20-05:00 · **Toggle Fast mode from the terminal header** (`codex/terminal-fast-toggle-20260907`)
  - Notes: Add a live Fast mode toggle beside the terminal pane menu
- **[feature]** `43f123c` · 2026-09-07T09:08:50-05:00 · **Agents have distinct characters and a searchable vertical menu** (`feat/agent-roster-20260907`)
  - Notes: Give agents shaped characters and a searchable vertical roster
- **[fix]** `e02cd26` · 2026-09-07T09:01:03-05:00 · **Chat resumes reliably and shows only supported controls** (`codex/chat-review-20260907`)
  - Notes: fix(chat): align controls and resume with provider capabilities
- **[feature]** `69ec21b` · 2026-09-07T08:47:13-05:00 · **Agent workspace keeps chat visible and offers provider model choices** (`feat/agent-workspace-20260907`)
  - Notes: Begin agent workspace with visible chat and provider model choices
- **[feature]** `7d26d6d` · 2026-09-07T08:39:57-05:00 · **OMP is available in the terminal provider picker** (`feat/omp-terminal`)
  - Notes: Add OMP to the terminal provider picker

### v0.3.66 — 2026-09-07T07:57:24-05:00 (`c76ea1a`)

- **[feature]** `922e9c7` · 2026-09-06T23:54:06-05:00 · **Preserve wrapped task text in terminal cards** (`codex/gui-consistency-20260906`)
- **[feature]** `88618d0` · 2026-09-06T23:48:47-05:00 · **Keep worker forms and terminal summaries readable** (`codex/gui-consistency-20260906`)
- **[feature]** `674bc34` · 2026-09-06T23:38:42-05:00 · **Polish canvas, search, terminal cards, and entry screens** (`codex/gui-consistency-20260906`)
- **[feature]** `8d0b046` · 2026-09-06T23:23:09-05:00 · **A newer release prevents an older candidate from staging** (`codex/streamline-release-pipeline`)
  - Notes: Stop superseded releases after publication readback
- **[fix]** `0d76d65` · 2026-09-06T23:19:49-05:00 · **Terminal completions notify and open the right workspace** (`fix/terminal-completion-notifications`)
  - Notes: fix(notifications): deliver terminal completion alerts
- **[feature]** `ebf6569` · 2026-09-06T23:18:23-05:00 · **Dev updates verify automatically and releases resume without rebuilding** (`codex/streamline-release-pipeline`)
  - Notes: Streamline dev verification and resumable release promotion
- **[speed]** `22c735a` · 2026-09-06T22:59:49-05:00 · **Terminal streaming uses less CPU and review commands respect timeouts** (`codex/performance-review-20260906`)
  - Notes: perf: bound terminal transport and subprocess resource costs
- **[fix]** `df79987` · 2026-09-06T22:39:53-05:00 · **Workspace selections keep their sidebar position** (`fix/sidebar-workspace-position`)
  - Notes: fix(sidebar): preserve workspace position when selected
- **[feature]** `c6f813d` · 2026-09-06T20:48:33-05:00 · **Named landings on dev and promote --change to ship them one at a time** (`feat/per-landing-promotion`)
  - Notes: docs(xtask): per-landing promotion design; docs(xtask): per-landing promotion implementation plan; feat(xtask): landing name rule and message; feat(xtask): parse, filter, resolve and render landings; fix(xtask): short landing SHAs through gate::short; feat(xtask): closeout lands one named merge commit on dev; feat(xtask): landings lists what dev has that main lacks; feat(xtask): promote --change ships chosen landings to main; fix(xtask): restore HEAD on every selective-promote failure path; docs: closeout --change, landings, and promote --change; fix(xtask): pin the dev tip a closeout merges and lands on; fix(xtask): put HEAD back on every selective-promote failure; docs(xtask): name the rollout escape hatch and refuse an empty --change; docs(spec): describe closeout as the fix wave leaves it

### v0.3.65 — 2026-09-06T17:54:30-05:00 (`265fd66`)

- **[feature]** `265fd66` · 2026-09-06T15:33:00-05:00 · **feat(terminals): the workspace just used rises to the top of its project**

### v0.3.64 — 2026-09-06T13:32:11-05:00 (`1447544`)

- **[fix]** `1447544` · 2026-09-06T12:54:32-05:00 · **fix(electron): let the renderer's own notifications through the permission handler**

### v0.3.63 — 2026-09-06T12:04:27-05:00 (`3683cf4`)

- **[fix]** `3683cf4` · 2026-09-06T11:34:12-05:00 · **fix(terminals): a refused shell reopen comes back as a standard terminal**
- **[fix]** `bfb02d9` · 2026-09-06T11:00:14-05:00 · **fix(terminals): an empty pane is not working, and a refused reopen says so**

### v0.3.62 — 2026-09-06T10:25:15-05:00 (`1f8579f`)

- **[fix]** `1f8579f` · 2026-09-06T08:28:04-05:00 · **fix(terminals): report Claude Code turns to the working dot through its hooks**
- **[fix]** `adc84be` · 2026-09-06T08:22:50-05:00 · **fix(renderer): file saved workspaces under their own project, not an ancestor folder**

### v0.3.61 — 2026-09-06T07:25:52-05:00 (`fb171d2`)

- **[fix]** `fb171d2` · 2026-09-06T07:15:28-05:00 · **fix(terminals): show the pane's name in its head again**

### v0.3.60 — 2026-09-05T23:09:11-05:00 (`847c167`)

- **[internal]** `847c167` · 2026-09-05T23:08:34-05:00 · **test(smoke): pin the terminal grid at its settled 16px gap and 10px radius**
- **[fix]** `764e368` · 2026-09-05T23:02:54-05:00 · **fix(terminals): scroll alt screens at xterm's line rate, not per wheel event**
- **[fix]** `36068fb` · 2026-09-05T22:55:30-05:00 · **fix(terminals): pane chrome measured against the reference**

### v0.3.59 — 2026-09-05T22:19:47-05:00 (`ba01fc8`)

- **[internal]** `ba01fc8` · 2026-09-05T22:17:33-05:00 · **test(host): count the inline hook-trust arguments in the Codex pane argv**
- **[internal]** `32a1a21` · 2026-09-05T22:15:08-05:00 · **chore(pipeline): settled-looks ledger and a closeout guard for it**
- **[fix]** `f549185` · 2026-09-05T22:05:19-05:00 · **fix(terminals): restore the selected pane's lifted head (settled look)**
- **[fix]** `24da267` · 2026-09-05T21:07:16-05:00 · **fix(host): confine send_to_pane on the pane server to the pass-off edge**
- **[internal]** `4a76ed6` · 2026-09-05T20:58:43-05:00 · **chore: rustfmt the pass-off changes and lock interface-common for the host**
- **[record]** `7765faa` · 2026-09-05T20:58:34-05:00 · **docs: pane pass-off tool — area docs, voice doc, as-built notes on the spec**
- **[feature]** `5be9f83` · 2026-09-05T20:57:48-05:00 · **feat(renderer): seat a tool-created pass-off pane beside its source**
- **[feature]** `5665135` · 2026-09-05T20:55:53-05:00 · **feat(host): iface MCP server for terminal panes with pass_off, read_pane, send_to_pane**
- **[feature]** `6e3035d` · 2026-09-05T20:44:24-05:00 · **feat(terminal): brief pass-off kind, pass-off source on summaries, pane environment, process ancestry**
- **[fix]** `ceccf44` · 2026-09-05T20:33:18-05:00 · **fix(dev): find the Dev host by executable, not by command line**
- **[record]** `1192876` · 2026-09-05T20:32:38-05:00 · **docs(specs): pane pass-off tool design**
- **[fix]** `1c1fdd9` · 2026-09-05T20:29:22-05:00 · **fix(dev): quit the Dev app cleanly, wait for its host, log the host's warnings**
- **[feature]** `05efc5d` · 2026-09-05T20:08:13-05:00 · **feat(renderer): attention tray — the bell that says what needs you, and where**

### v0.3.58 — 2026-09-05T20:01:00-05:00 (`259f8eb`)

- **[fix]** `259f8eb` · 2026-09-05T19:52:02-05:00 · **fix(terminals): Layout toggle slides Flip and Tidy out, as the old More did**
- **[fix]** `4f3853d` · 2026-09-05T19:47:08-05:00 · **fix(terminals): one four-square layout menu in the toolbar**
- **[feature]** `600bf25` · 2026-09-05T19:31:50-05:00 · **feat(terminals): controls in the toolbar, quieter tiles, flat surface switch**
- **[feature]** `3fd0d81` · 2026-09-05T19:07:08-05:00 · **feat(sidebar): one header row, single-line rows, quieter chips**
- **[fix]** `dfd5615` · 2026-09-05T18:44:05-05:00 · **fix(sidebar): quieter count chips, shared by project headers**

### v0.3.57 — 2026-09-05T18:32:17-05:00 (`cc9acfc`)

- **[fix]** `cc9acfc` · 2026-09-05T18:29:06-05:00 · **fix(terminals): resume Grok reopens from Grok's session store**

### v0.3.56 — 2026-09-05T17:20:20-05:00 (`0ee2b22`)

- **[fix]** `0ee2b22` · 2026-09-05T17:20:10-05:00 · **fix(sidebar): keep projects in name order; lift live workspaces**
- **[fix]** `6b21d35` · 2026-09-05T17:03:37-05:00 · **fix(terminals): re-assert bracketed paste when a claude or codex pane remounts**

### v0.3.55 — 2026-09-05T15:22:05-05:00 (`a5752b5`)

- **[fix]** `a5752b5` · 2026-09-05T15:20:04-05:00 · **fix(renderer): cmd-arrows, flip glyph, working halo, quieter pane chrome**
- **[feature]** `3bf72e4` · 2026-09-05T15:15:25-05:00 · **feat(updater): check as soon as a feed is staged**
- **[feature]** `61cf69c` · 2026-09-05T15:04:10-05:00 · **feat(updater): check on window focus and every 15 minutes**
- **[fix]** `0c26b4f` · 2026-09-05T12:21:46-05:00 · **fix(terminals): restore alt-screen wheel for Grok Build panes**

### v0.3.54 — 2026-09-05T09:31:04-05:00 (`f451bd2`)

- **[fix]** `f451bd2` · 2026-09-05T09:26:33-05:00 · **fix(terminals): do not file every workspace under /**

### v0.3.53 — 2026-09-05T08:31:28-05:00 (`7fdd780`)

- **[fix]** `a50930e` · 2026-09-05T01:02:24-05:00 · **fix(terminals): trust Interface's own Codex hook inline so panes resume again**
- **[fix]** `7fdd780` · 2026-09-05T00:42:33-05:00 · **fix(terminal): lift only the focused pane's head**
- **[internal]** `64dd01c` · 2026-09-05T00:26:52-05:00 · **chore(settings): allow cargo xtask promote in agent sessions**

### v0.3.52 — 2026-09-05T00:21:04-05:00 (`1b8f41b`)

- **[feature]** `1b8f41b` · 2026-09-05T00:15:06-05:00 · **feat(terminals): match the selected pane chrome to the reference Codex pane**
- **[feature]** `fa0fc80` · 2026-09-05T00:04:59-05:00 · **feat(sidebar): fill the workspace count chip**
- **[feature]** `0376f47` · 2026-09-04T23:00:23-05:00 · **feat(sidebar): ring the workspace row's terminal count**
- **[feature]** `8996723` · 2026-09-04T21:02:08-05:00 · **feat(panes): diagonal-arrow maximize and compact header glyphs**
- **[fix]** `3c77962` · 2026-09-04T21:01:42-05:00 · **fix(terminals): use Codex OSC9 notifications instead of bare BEL**
- **[record]** `f3b882c` · 2026-09-04T19:53:19-05:00 · **docs: drop Project Plans, the board and CodeViewBar from the maps**
- **[feature]** `9964151` · 2026-09-04T19:53:04-05:00 · **feat(host): delete interface-plans and the plan-run half of orchestration**
- **[fix]** `7fae122` · 2026-09-04T19:52:44-05:00 · **fix(electron): drop the eight plan and plan-run channels; protocol v6**
- **[feature]** `47f46d5` · 2026-09-04T19:52:36-05:00 · **feat(renderer): remove Project Plans, the sessions board, and the Code view bar**
- **[fix]** `deb9306` · 2026-09-04T19:29:00-05:00 · **fix(terminals): grey pane dot when the shell is idle**
- **[fix]** `8688fb1` · 2026-09-04T19:29:00-05:00 · **fix(sidebar): keep the workspace row's working dot solid**
- **[fix]** `f7d4d1a` · 2026-09-04T19:23:50-05:00 · **fix(terminals): resume Codex started from a shell pane**
- **[fix]** `d406677` · 2026-09-04T19:13:21-05:00 · **fix(terminals): paste dropped image paths instead of covering them**
- **[feature]** `757214c` · 2026-09-04T19:09:56-05:00 · **feat(renderer): let errors fade out on their own**
- **[feature]** `224f4c2` · 2026-09-04T19:03:06-05:00 · **feat(terminals): let a workspace split past six panes**
- **[feature]** `ee670a3` · 2026-09-04T19:02:37-05:00 · **feat(terminals): tuck Flip, Tidy and New behind one More toggle**
- **[feature]** `24f3ffd` · 2026-09-04T18:58:25-05:00 · **feat(renderer): show an update pill next to usage while an update exists**

### v0.3.51 — 2026-09-04T18:37:15-05:00 (`182d697`)

- **[fix]** `182d697` · 2026-09-04T18:36:34-05:00 · **fix(release): rebuild stamped outputs and acknowledge smoke data**
- **[fix]** `4995959` · 2026-09-04T18:04:28-05:00 · **fix(terminals): recognize manual providers without focus activity flashes**
- **[speed]** `77857cc` · 2026-09-04T16:50:08-05:00 · **perf: bound hot paths and simplify renderer architecture**
- **[fix]** `639a752` · 2026-09-04T16:18:09-05:00 · **fix(terminals): name Codex panes from chat titles**
- **[fix]** `74fa524` · 2026-09-04T14:46:05-05:00 · **fix(terminals): refresh provider executables for new panes**
- **[fix]** `40ac51c` · 2026-09-04T14:35:56-05:00 · **fix(chat): launch project chats in their selected directory**
- **[fix]** `a3494e8` · 2026-09-04T13:23:12-05:00 · **fix(xtask): keep managed checkouts out of worktree-prune; drop the shared target dir**
- **[internal]** `ce0b5cb` · 2026-09-04T13:22:06-05:00 · **refactor(tests): link each crate's integration tests as one binary**
- **[internal]** `70d79bb` · 2026-09-04T13:19:09-05:00 · **refactor(tests): move inline test modules beside their code**
- **[internal]** `b816dcd` · 2026-09-04T13:18:23-05:00 · **chore(xtask): add closeout, dev-restart and worktree-prune**
- **[internal]** `ec3bb0f` · 2026-09-04T13:12:15-05:00 · **chore: allow the closeout commands, pin CI's target dir, drop the label probe**
- **[feature]** `739f3b6` · 2026-09-04T13:06:23-05:00 · **build: quiet the gate, share the target dir, trim debuginfo**
- **[record]** `e5dd659` · 2026-09-04T12:31:16-05:00 · **docs: shrink the agent reading set**
- **[record]** `f4a3f15` · 2026-09-04T12:17:29-05:00 · **docs(release): say the seed copies tracked files**
- **[fix]** `23912c3` · 2026-09-04T12:06:29-05:00 · **fix(xtask): name the seeded file count before printing it**
- **[fix]** `394065b` · 2026-09-04T12:05:21-05:00 · **fix(xtask): seed the release worktree from git's own file list**
- **[fix]** `cfb2c04` · 2026-09-04T11:40:54-05:00 · **fix(xtask): stop claiming artifacts the release worktree never held**
- **[fix]** `656cbe7` · 2026-09-04T11:40:15-05:00 · **fix(scripts): report an expired redraw wait as a timeout**
- **[fix]** `eadb8ce` · 2026-09-04T11:38:47-05:00 · **fix(xtask): retry the release worktree's git on a cancelled mmap**
- **[fix]** `a5b3f29` · 2026-09-04T11:37:45-05:00 · **fix(xtask): say which gate stamp the release worktree inherits**
- **[fix]** `6c53d56` · 2026-09-04T11:36:33-05:00 · **fix(xtask): seed a new release worktree instead of checking it out**
- **[record]** `b9a6680` · 2026-09-04T11:34:16-05:00 · **docs(release): default the release worktree outside iCloud**

### v0.3.50 — 2026-09-04T10:42:45-05:00 (`689e47f`)

- **[feature]** `689e47f` · 2026-09-04T10:08:27-05:00 · **ci: record which gate case --ignore-scripts skips**
- **[record]** `16f06db` · 2026-09-04T10:08:27-05:00 · **docs(release): say how to reach origin/dev and what --no-push leaves**
- **[fix]** `8ba07d3` · 2026-09-04T10:08:27-05:00 · **fix(xtask): let a promotion resume all the way through packaging**
- **[fix]** `9a0e71e` · 2026-09-04T09:39:26-05:00 · **fix(xtask): tell the operator what actually landed**
- **[fix]** `43b83f7` · 2026-09-04T09:30:18-05:00 · **fix(xtask): pin the promoted commit and widen the resume rule**
- **[fix]** `87321fa` · 2026-09-04T09:23:15-05:00 · **fix(scripts): refuse a second dev supervisor before it builds**
- **[internal]** `e10a961` · 2026-09-04T09:23:06-05:00 · **test(scripts): make repeated pin literals able to fail**
- **[fix]** `11c8a2b` · 2026-09-04T09:16:33-05:00 · **fix(xtask): close the retry, stamp, and half-promote gaps**
- **[internal]** `efe9fb3` · 2026-09-04T09:01:34-05:00 · **test(scripts): cover the composer label checks the pins rewrote**
- **[internal]** `954b16d` · 2026-09-04T09:00:23-05:00 · **test(scripts): keep the pin test file plain text**
- **[internal]** `fa39b93` · 2026-09-04T08:59:28-05:00 · **chore(scripts): forward check --changed and promote to xtask**
- **[fix]** `3df650b` · 2026-09-04T08:58:49-05:00 · **fix(scripts): build the dev voice helper only when it changed**
- **[internal]** `719df1d` · 2026-09-04T08:55:18-05:00 · **test(scripts): pin the packaged smoke probe to the renderer**
- **[record]** `b3cfe3a` · 2026-09-04T08:54:01-05:00 · **docs(release): tighten closeout wording and pin the gate's Node**
- **[record]** `e9031af` · 2026-09-04T08:53:08-05:00 · **docs(xtask): explain the new gate and release flags in --help**
- **[feature]** `52e95ad` · 2026-09-04T08:51:56-05:00 · **feat(xtask): add promote for the whole approved release**
- **[feature]** `daf1404` · 2026-09-04T08:47:24-05:00 · **feat(xtask): retry a known flake and a timed-out smoke once**
- **[feature]** `bc1cc79` · 2026-09-04T08:44:37-05:00 · **feat(xtask): stamp the gate and let package-mac trust it**
- **[feature]** `327843d` · 2026-09-04T08:42:18-05:00 · **feat(xtask): scope check to the changed side of the tree**
- **[feature]** `5588831` · 2026-09-04T08:40:56-05:00 · **ci: add a manual macOS gate workflow**
- **[record]** `0aaad5b` · 2026-09-04T08:40:48-05:00 · **docs(codemap): document the scoped gate, stamp and promote**
- **[record]** `07586d3` · 2026-09-04T08:40:41-05:00 · **docs(release): make promote the closeout path to main**
- **[internal]** `7ef2507` · 2026-09-04T07:59:19-05:00 · **test(smoke): probe the terminal pane and its siblings as they are now**

### v0.3.49 — 2026-09-04T07:22:20-05:00 (`69eadfc`)

- **[fix]** `69eadfc` · 2026-09-03T23:20:47-05:00 · **fix(renderer): bring the voice UI onto the theme guards**
- **[fix]** `9dfaa3a` · 2026-09-03T23:10:56-05:00 · **fix(renderer): stop four small answers from being wrong**
- **[fix]** `bee1ebf` · 2026-09-03T23:08:18-05:00 · **fix(renderer): name the flip card's dot and the two search rows**
- **[record]** `8bbf72c` · 2026-09-03T23:06:28-05:00 · **docs(codemap): describe the renderer as it is after the UI plan**
- **[fix]** `9e0514a` · 2026-09-03T23:05:09-05:00 · **fix(renderer): give the palette one open-state source**
- **[internal]** `522e4ee` · 2026-09-03T22:59:09-05:00 · **test(smoke): probe the launcher and review dialog as they are now**
- **[internal]** `fddc24d` · 2026-09-03T22:38:49-05:00 · **test(theme): close the leading guard's font-shorthand hole**
- **[fix]** `b4dbd49` · 2026-09-03T22:18:40-05:00 · **fix(renderer): close three strays the surface tasks left behind**
- **[look]** `a8dc2a8` · 2026-09-03T22:17:33-05:00 · **style(renderer): put the sub-24px insets back on the space ramp**
- **[internal]** `4f2015e` · 2026-09-03T22:14:46-05:00 · **test(theme): drop the fallback on a token :root already declares**
- **[internal]** `7eb5f5b` · 2026-09-03T22:14:01-05:00 · **test(theme): track a caps label by one amount, and small text by none**
- **[internal]** `04f6da2` · 2026-09-03T22:12:37-05:00 · **test(theme): set every line on one of the three leading tiers**
- **[internal]** `fce8a9a` · 2026-09-03T22:05:26-05:00 · **test(theme): draw every corner from the radius system**
- **[fix]** `c08c005` · 2026-09-03T22:02:26-05:00 · **fix(terminals): end a browsed folder with the session that chose it**
- **[internal]** `5b23d9d` · 2026-09-03T22:00:30-05:00 · **test(theme): catch a literal weight in the font shorthand too**
- **[internal]** `efc926a` · 2026-09-03T21:58:37-05:00 · **test(theme): hold disabled to a colour, not an opacity**
- **[look]** `a3ee989` · 2026-09-03T21:50:39-05:00 · **style(theme): answer a press on the launcher and review-dialog buttons**
- **[fix]** `9a2ed4b` · 2026-09-03T21:50:31-05:00 · **fix(terminals): agree on one launch folder, and wait to take focus**
- **[look]** `8728ddf` · 2026-09-03T21:28:34-05:00 · **style(terminals): retire two stale notes left by the surface rework**
- **[fix]** `ce1132a` · 2026-09-03T21:27:04-05:00 · **fix(terminals): announce a terminal that needs you or has failed**
- **[fix]** `7b97ce7` · 2026-09-03T21:25:38-05:00 · **fix(terminals): put Code's view bar in the terminal toolbar**
- **[internal]** `6cc50a6` · 2026-09-03T21:23:03-05:00 · **refactor(terminals): move adversarial review into its own dialog**
- **[record]** `daf13a4` · 2026-09-03T21:18:15-05:00 · **docs: describe the voice assistant and its plans**
- **[fix]** `b2fbc73` · 2026-09-03T21:15:30-05:00 · **fix(terminals): give the launcher Escape and an initial focus**
- **[fix]** `f7ba118` · 2026-09-03T21:14:00-05:00 · **fix(terminals): hold one name per object and drop "session"**
- **[look]** `b52061f` · 2026-09-03T21:11:58-05:00 · **style(terminals): hold two icon-button sizes and one glyph per meaning**
- **[fix]** `86fc99b` · 2026-09-03T21:09:58-05:00 · **fix(renderer): put the picker's field shape and the two reads in one place**
- **[internal]** `abf797a` · 2026-09-03T21:08:51-05:00 · **refactor(terminals): delete the unreachable card overview**
- **[fix]** `2fd5170` · 2026-09-03T21:07:57-05:00 · **fix(voice): recover from error and keep interrupted turns audible**
- **[fix]** `c760ac9` · 2026-09-03T21:02:11-05:00 · **fix(renderer): guard orchestration approval and confirm dialogs from ui_act**
- **[fix]** `420cc65` · 2026-09-03T21:00:32-05:00 · **fix(renderer): stop voice re-selection, guard approvals from ui_act, show notify**
- **[fix]** `e948d43` · 2026-09-03T20:58:18-05:00 · **fix(renderer): name the file picker for what it does, and four small corrections**
- **[fix]** `9535f7e` · 2026-09-03T20:58:09-05:00 · **fix(renderer): keep every board section on screen, and Needs attention open**
- **[fix]** `3b63a66` · 2026-09-03T20:49:03-05:00 · **fix(composer): give the bypass card one name, and pin the effort gate**
- **[internal]** `cd1ab6a` · 2026-09-03T20:49:03-05:00 · **chore(theme): retire --bg and make the motion and spinner notes true**
- **[fix]** `605ae85` · 2026-09-03T20:48:50-05:00 · **fix(renderer): settle the danger hover, the skeleton cadence and h4-h6**
- **[internal]** `818ad31` · 2026-09-03T20:42:23-05:00 · **refactor(orchestration): fold the host's read-only surfaces behind one door**
- **[record]** `e19061b` · 2026-09-03T20:40:52-05:00 · **docs(renderer): correct two board comments the renames left stale**
- **[internal]** `0787390` · 2026-09-03T20:40:30-05:00 · **refactor(orchestration): move the last seventeen pickers onto FormSelect**
- **[internal]** `0fa0ef7` · 2026-09-03T20:40:09-05:00 · **refactor(renderer): fold Quick Open into the palette and settle one overlay recipe**
- **[fix]** `913d457` · 2026-09-03T20:37:45-05:00 · **fix(renderer): show voice sessions on the Chat surface**
- **[internal]** `2bdd7a5` · 2026-09-03T20:33:57-05:00 · **refactor(plans): lift CanvasSelect into FormSelect and use it on the plan page**
- **[feature]** `f8eb3da` · 2026-09-03T20:33:16-05:00 · **feat(host): voice service, MCP tools, and UI bridge**
- **[look]** `62bdfc5` · 2026-09-03T20:30:42-05:00 · **style(transcript): keep the code-block header at the 26px it claims**
- **[feature]** `a14d737` · 2026-09-03T20:29:55-05:00 · **feat(renderer): give Code's second axis a visible control**
- **[look]** `9f81fc0` · 2026-09-03T20:28:50-05:00 · **style(renderer): move the last motion literal and the last legacy alias**
- **[feature]** `35a4c1b` · 2026-09-03T20:27:55-05:00 · **feat(canvas): give the canvas a door and the app's own status words**
- **[fix]** `ab29f8d` · 2026-09-03T20:27:40-05:00 · **fix(renderer): draw the xAI mark at the other provider marks' ink scale**
- **[internal]** `606c41d` · 2026-09-03T20:27:34-05:00 · **refactor(renderer): delete 26 CSS rules nothing renders**
- **[fix]** `22467b9` · 2026-09-03T20:26:51-05:00 · **fix(renderer): keep one attention indicator, in the toolbar**
- **[fix]** `9122ed5` · 2026-09-03T20:26:06-05:00 · **fix(renderer): make Settings a page instead of a page-shaped dialog**
- **[fix]** `92d18c7` · 2026-09-03T20:25:08-05:00 · **fix(transcript): let the density dial show which rung it is on**
- **[fix]** `b6923d0` · 2026-09-03T20:24:05-05:00 · **fix(renderer): make the destructive buttons the loud ones**
- **[internal]** `548b456` · 2026-09-03T20:23:40-05:00 · **refactor(renderer): make the board one list of rows, not three tiers of chrome**
- **[internal]** `a9609a9` · 2026-09-03T20:23:17-05:00 · **refactor(renderer): draw the agent surfaces' eight pills as one badge**
- **[fix]** `86557af` · 2026-09-03T20:22:37-05:00 · **fix(renderer): name the access modes by what they do to an action**
- **[fix]** `0029578` · 2026-09-03T20:21:33-05:00 · **fix(renderer): give five overloaded glyphs one meaning each**
- **[internal]** `171f781` · 2026-09-03T20:20:10-05:00 · **refactor(renderer): spend the motion budget only where it is watched**
- **[record]** `c202b56` · 2026-09-03T20:19:56-05:00 · **docs: update channel counts for the voice surface**
- **[fix]** `a44fbfa` · 2026-09-03T20:19:56-05:00 · **fix(renderer): settle the sidebar's names, hide action and footer**
- **[look]** `d498a1f` · 2026-09-03T20:19:43-05:00 · **style(renderer): read the motion tokens without fallbacks**
- **[fix]** `eb4476e` · 2026-09-03T20:19:41-05:00 · **fix(orchestration): announce the armed step and confirm two destructive acts**
- **[fix]** `ac3ce37` · 2026-09-03T20:18:14-05:00 · **fix(renderer): state disabled as a colour, not an opacity**
- **[feature]** `b0fa21b` · 2026-09-03T20:17:32-05:00 · **feat(renderer): voice toggle, indicator, and UI bridge**
- **[fix]** `5f71bbf` · 2026-09-03T20:17:31-05:00 · **fix(renderer): paint Send in the app's primary-action colour**
- **[internal]** `795268d` · 2026-09-03T20:17:18-05:00 · **refactor(plans): remount only the plan's forms on a CAS bump**
- **[internal]** `6342714` · 2026-09-03T20:16:50-05:00 · **test(theme): make the frost guard reach every root, not every file**
- **[fix]** `114898c` · 2026-09-03T20:16:26-05:00 · **fix(transcript): stop marking inert rows with a fold chevron**
- **[fix]** `05ebf5d` · 2026-09-03T20:14:50-05:00 · **fix(renderer): raise five controls to the 24px floor**
- **[look]** `fa44074` · 2026-09-03T20:13:40-05:00 · **style(transcript): give links a cue and a new turn its own beat**
- **[feature]** `ec90229` · 2026-09-03T20:12:21-05:00 · **feat(engine-claude): voice session prompt and MCP attachment**
- **[fix]** `22a67d1` · 2026-09-03T20:12:08-05:00 · **fix(renderer): say what the agent is working on**
- **[feature]** `d7f17f6` · 2026-09-03T20:09:46-05:00 · **feat(contracts): add voice session kind, DTOs, and channels**
- **[feature]** `50b858b` · 2026-09-03T20:09:23-05:00 · **feat(electron): voice channels, permission handler, screenshot bridge**
- **[feature]** `60c8b6a` · 2026-09-03T20:07:52-05:00 · **feat(voice): add interface-voice protocol, chunker, and tail buffer**
- **[feature]** `5c82f5b` · 2026-09-03T20:06:05-05:00 · **build: bundle interface-voice helper and microphone usage description**
- **[feature]** `0b8b4f4` · 2026-09-03T20:02:44-05:00 · **feat(voice): add interface-voice Swift speech helper and design docs**
- **[look]** `18ffd6b` · 2026-09-03T19:59:56-05:00 · **style(theme): tidy two comments from the self-review**
- **[fix]** `1e1ec3e` · 2026-09-03T19:58:59-05:00 · **fix(theme): give links the app's focus ring**
- **[record]** `8936db6` · 2026-09-03T19:58:25-05:00 · **docs(theme): make the motion claims true and drop a stale note**
- **[internal]** `5224f37` · 2026-09-03T19:57:54-05:00 · **refactor(theme): draw the Hermes watermark once, on one plate**
- **[fix]** `42ae699` · 2026-09-03T19:57:00-05:00 · **fix(renderer): raise the status bar and tile recap close to 24px**
- **[look]** `d02f637` · 2026-09-03T19:55:32-05:00 · **style(theme): add leading and tracking tokens and use them here**
- **[internal]** `33ebccd` · 2026-09-03T19:53:16-05:00 · **refactor(theme): delete sixteen tokens nothing reads**
- **[internal]** `f70019e` · 2026-09-03T19:50:27-05:00 · **refactor(renderer): frost floating surfaces by one class**
- **[record]** `c6fcd2b` · 2026-09-03T19:50:27-05:00 · **docs(agents): drop permission guardrails from AGENTS.md**
- **[look]** `5b0a365` · 2026-09-03T19:47:28-05:00 · **style(theme): make a disabled button a colour, not an opacity**
- **[look]** `8dfa598` · 2026-09-03T19:47:02-05:00 · **style(theme): extend the press rule and add the badge primitive**
- **[look]** `23f8a44` · 2026-09-03T19:44:54-05:00 · **style(theme): put the em-sized browser defaults on the ramps**
- **[fix]** `4df7f2e` · 2026-09-03T19:43:58-05:00 · **fix(renderer): declare the codicon glyphs the app already emits**
- **[fix]** `f12e889` · 2026-09-03T19:41:27-05:00 · **fix(renderer): point four var() reads at tokens that exist**
- **[fix]** `7196739` · 2026-09-03T18:50:47-05:00 · **fix(terminals): resume Codex panes on their own thread**
- **[fix]** `3572d6f` · 2026-09-03T14:39:46-05:00 · **fix(terminals): resume shell-started Claude sessions**
- **[fix]** `daf1e5e` · 2026-09-03T08:09:21-05:00 · **fix(terminals): preserve screenshot drop source**
- **[fix]** `c60e68b` · 2026-09-03T07:28:45-05:00 · **fix(renderer): protect active projects and square xAI mark**

### v0.3.47 — 2026-09-02T23:01:08-05:00 (`b37be78`)

- **[fix]** `b37be78` · 2026-09-02T22:42:46-05:00 · **fix(terminals): replace Gemini CLI with Antigravity**
- **[fix]** `93c54ee` · 2026-09-02T22:40:10-05:00 · **fix(renderer): use SpaceXAI glyph for Grok**
- **[fix]** `25a0eb1` · 2026-09-02T22:28:29-05:00 · **fix(renderer): use xAI mark for Grok**
- **[fix]** `5a58be5` · 2026-09-02T21:33:25-05:00 · **fix(sidebar): allow archiving idle terminal projects**

### v0.3.46 — 2026-09-02T20:20:04-05:00 (`4ce45ff`)

- **[fix]** `4ce45ff` · 2026-09-02T19:58:56-05:00 · **Fix Claude terminal autoresume across worktrees**
- **[fix]** `4eeae68` · 2026-09-02T16:36:15-05:00 · **fix(renderer): retain inherited terminal icons**
- **[fix]** `a57ca0f` · 2026-09-02T16:24:31-05:00 · **fix(agent): make worktrees explicit opt-in**
- **[fix]** `850da09` · 2026-09-02T16:15:20-05:00 · **fix(renderer): inherit terminal provider icons**
- **[fix]** `7cb6dd3` · 2026-09-02T15:28:11-05:00 · **fix(sidebar): pulse active workspace dots**
- **[fix]** `502b50e` · 2026-09-02T09:02:34-05:00 · **fix(terminal): restore Claude sessions and workspace titles**
- **[internal]** `649a99f` · 2026-09-02T07:37:19-05:00 · **chore: bump to 0.3.44**
- **[look]** `ab5f9e7` · 2026-09-02T01:31:45-05:00 · **style(renderer): move the last two 100ms transitions onto --motion-fast**
- **[fix]** `d728b36` · 2026-09-02T01:31:38-05:00 · **fix(renderer): give the launcher rows the menus' one tab stop**
- **[fix]** `f4c8c16` · 2026-09-02T01:31:31-05:00 · **fix(renderer): let the agent dialog always carry its own stylesheet**
- **[fix]** `d927720` · 2026-09-02T01:08:39-05:00 · **fix(renderer): name the flipped pane card by what the card shows**
- **[look]** `48963f1` · 2026-09-02T01:08:39-05:00 · **style(renderer): let inline code keep scaling with its parent**
- **[fix]** `856770b` · 2026-09-02T01:02:58-05:00 · **fix(renderer): skip disabled items in the row menu's arrow keys too**
- **[fix]** `5a725fc` · 2026-09-02T01:02:58-05:00 · **fix(renderer): keep the terminal menu tabbable and its cards truthful**
- **[internal]** `f5a54e4` · 2026-09-02T01:02:44-05:00 · **test(renderer): replay a multi-byte row through the boundary walk**
- **[fix]** `1d8aa1a` · 2026-09-02T01:02:43-05:00 · **fix(renderer): give the unavailable capability pill its own fill**
- **[speed]** `0851d54` · 2026-09-02T01:02:33-05:00 · **perf(renderer): bump the catalogue only when a real one departs**
- **[fix]** `5b8807d` · 2026-09-02T01:02:22-05:00 · **fix(renderer): make each injectStyles opt-out reach a caller or go**
- **[look]** `02e8d40` · 2026-09-02T00:52:15-05:00 · **style(renderer): draw every focus ring from the shared tokens**
- **[look]** `3398e2c` · 2026-09-02T00:50:00-05:00 · **style(renderer): put every stylesheet on the declared type ramp**
- **[fix]** `946ed2a` · 2026-09-02T00:36:44-05:00 · **fix(renderer): review minors — clip bound, menu focus, library confirm, copy**
- **[fix]** `1db0935` · 2026-09-02T00:36:44-05:00 · **fix(renderer): bump the catalogue revision when a session holding one leaves**
- **[feature]** `6695cb9` · 2026-09-02T00:36:31-05:00 · **revert(transcript): drop content-visibility from the measured row**
- **[fix]** `3e38aed` · 2026-09-02T00:36:21-05:00 · **fix(transcript): keep the import parser off fenced and unclosed prose**
- **[look]** `a12c1cf` · 2026-09-02T00:19:57-05:00 · **style(renderer): keep menuKeyNav's doc comment on menuKeyNav**
- **[fix]** `3b40a3d` · 2026-09-02T00:19:38-05:00 · **fix(transcript): hand back a row whose markers were only talked about**
- **[fix]** `3a29eea` · 2026-09-02T00:19:09-05:00 · **fix(board): keep the memo'd board's card ages ticking**
- **[fix]** `25572a3` · 2026-09-02T00:17:45-05:00 · **fix(transcript): contain the row at the node the pane measures**
- **[speed]** `b502af1` · 2026-09-02T00:16:47-05:00 · **perf(renderer): mint choicesFor on the catalogue revision, not every event frame**
- **[speed]** `1769451` · 2026-09-02T00:16:17-05:00 · **perf(board): reuse last frame's card when nothing it prints changed**
- **[speed]** `42c52d7` · 2026-09-02T00:14:40-05:00 · **perf(renderer): keep field-equal diff stats and peek facts across frames**
- **[speed]** `2449a70` · 2026-09-02T00:14:29-05:00 · **perf(transcript): take the reveal step on the commit that brought the text**
- **[speed]** `4347303` · 2026-09-02T00:13:21-05:00 · **perf(renderer): cache the config view's log walks and reuse the view per session**
- **[speed]** `cb1c94c` · 2026-09-02T00:10:51-05:00 · **perf(renderer): memo the plans and terminal surfaces; hoist their inline props**
- **[internal]** `32f4372` · 2026-09-02T00:10:51-05:00 · **test(viewmodels): pass only cwd members to workspaceSidebarTitle**
- **[speed]** `6bb21e6` · 2026-09-02T00:10:12-05:00 · **perf(transcript): give the row a memo boundary and let offscreen rows skip**
- **[speed]** `572a978` · 2026-09-02T00:08:50-05:00 · **perf(canvas): memoize the edge array and memo AgentCanvas**
- **[speed]** `e6e32b4` · 2026-09-02T00:07:58-05:00 · **perf(renderer): memo the board and skip unchanged workspace reconciles**
- **[feature]** `1e96164` · 2026-09-02T00:06:17-05:00 · **feat(terminal): put folder, state and last output on the cards**
- **[fix]** `229de2c` · 2026-09-02T00:06:08-05:00 · **fix(a11y): give the terminal manager menu one tab stop**
- **[speed]** `150bd30` · 2026-09-02T00:06:02-05:00 · **perf(terminal): render xterm on WebGL and stop the cursor blinking**
- **[look]** `8f7d77a` · 2026-09-02T00:05:53-05:00 · **style(terminal): stop covered tabs painting, drop dead CSS**
- **[speed]** `73cdef9` · 2026-09-02T00:05:43-05:00 · **perf(terminal): skip ground-state bytes when advancing replay**
- **[speed]** `42ecdc1` · 2026-09-02T00:05:20-05:00 · **perf(workspace): reconcile returns its input when no key was dropped**
- **[fix]** `d883165` · 2026-09-02T00:04:47-05:00 · **fix(renderer): centre the Chat and board zero-states; codicons in the Settings rail**
- **[fix]** `96485d2` · 2026-09-02T00:04:44-05:00 · **fix(transcript): token the steer pill, class the slash glyph, name the pills**
- **[feature]** `e20a22e` · 2026-09-02T00:02:40-05:00 · **feat(transcript): make code and tool output reachable, and copyable**
- **[fix]** `321176f` · 2026-09-02T00:02:28-05:00 · **fix(sidebar): name a minted workspace row after its folder, not a pane title**
- **[fix]** `9bd67ad` · 2026-09-02T00:01:39-05:00 · **fix(theme): one focus ring, a press, and quiet controls that are visible**
- **[speed]** `85aaf7d` · 2026-09-02T00:01:23-05:00 · **perf(ui): memo the tile access rows, guard duplicate style injection**
- **[fix]** `5ea9590` · 2026-09-02T00:01:13-05:00 · **fix(steering): drop the var() for tokens nothing declares**
- **[fix]** `4a1f031` · 2026-09-02T00:01:05-05:00 · **fix(panels): read badge pills against --badge-fg**
- **[fix]** `d73d7d8` · 2026-09-01T23:59:57-05:00 · **fix(transcript): stop a tool row ellipsising its name before the path**
- **[fix]** `4a7ea4c` · 2026-09-01T23:59:50-05:00 · **fix(renderer): sentence-case copy, labelled roles, shortcut rows, roving menus**
- **[feature]** `d61f509` · 2026-09-01T23:58:07-05:00 · **feat(transcript): read an imported session's markers back as rows**
- **[fix]** `6c92f5f` · 2026-09-01T23:56:37-05:00 · **fix(library): confirm Clean up before removing a managed worktree**
- **[speed]** `087eee7` · 2026-09-01T23:55:02-05:00 · **perf(renderer): clip the board activity line from a bounded prefix**
- **[fix]** `beae546` · 2026-09-01T23:54:12-05:00 · **fix(renderer): let a focused shell keep Ctrl+P, Ctrl+N and Ctrl+Tab**
- **[internal]** `f4fd7b8` · 2026-09-01T17:15:25-05:00 · **chore: one canonical timestamp renderer, stricter TypeScript, drop dead renderer code**
- **[feature]** `dbc5a66` · 2026-09-01T16:48:23-05:00 · **feat(sidebar): drop the working glyph ticker, keep the solid dot**
- **[record]** `36b029c` · 2026-09-01T16:45:40-05:00 · **docs: bring CODEMAP and the area AGENTS files back in line with the cleanup**
- **[feature]** `b0f804b` · 2026-09-01T16:41:21-05:00 · **feat(sidebar): glyph ticker beside a solid working dot**
- **[speed]** `03c2f07` · 2026-09-01T16:37:33-05:00 · **perf(engines): reap cancelled Codex control ops, cache the Claude listing, share the transport half**
- **[speed]** `52c321b` · 2026-09-01T16:31:58-05:00 · **perf(persist): fix the numstat and prune regressions, one path normalizer, leaner stores**
- **[speed]** `864779e` · 2026-09-01T16:25:26-05:00 · **perf(renderer): stop per-frame refits and scans, drain hidden windows on a timer, fold duplicates**
- **[speed]** `6a674e5` · 2026-09-01T16:24:59-05:00 · **perf(host): memoize the orchestration snapshot, coalesce pushes through one coordinator**
- **[speed]** `63d4c8d` · 2026-09-01T16:18:03-05:00 · **perf(core): drop per-validate allocations, the regex timestamp gate and the dead control codec**
- **[feature]** `c5d1a28` · 2026-09-01T16:14:22-05:00 · **feat(sidebar): turning half-lit sphere beside a working workspace's dot**
- **[speed]** `78bcdb1` · 2026-09-01T16:05:08-05:00 · **perf(iface): cache the transcript paragraph, fix the layout revision race, fold private-store helpers**
- **[internal]** `29aaa12` · 2026-09-01T15:55:26-05:00 · **chore(electron): incremental main build, drop dead shared validators and workspace deps**
- **[feature]** `a3d9862` · 2026-09-01T15:25:25-05:00 · **feat(notify): OS notification and chime when any agent needs input**
- **[internal]** `1685fdf` · 2026-09-01T14:49:57-05:00 · **chore(release): 0.3.43**
- **[fix]** `2c2f021` · 2026-09-01T11:50:48-05:00 · **fix(terminal): let agent panes paste images natively via Ctrl+V**
- **[fix]** `90fdff6` · 2026-09-01T11:49:34-05:00 · **fix(agent): seed the agent form once per open; task-less agent shows its own zero-state**
- **[feature]** `6da935c` · 2026-09-01T11:45:33-05:00 · **feat(agent): agents menu on the Agent surface**
- **[fix]** `7c59a88` · 2026-09-01T11:31:44-05:00 · **fix(terminal): show image chips in the alternate buffer**
- **[feature]** `39ae0e9` · 2026-09-01T11:22:43-05:00 · **feat(terminal): fade splitter bars in on hover**
- **[record]** `2fb74c0` · 2026-09-01T10:57:43-05:00 · **docs(codemap): record the dev-app FSEvents restart trap**
- **[fix]** `f6376c5` · 2026-09-01T10:51:55-05:00 · **fix(dev): stop the Dev app restart loop from replayed FSEvents**
- **[fix]** `b1355c8` · 2026-09-01T09:51:54-05:00 · **fix(terminal): hide splitter bars until hover**
- **[fix]** `b8d973d` · 2026-09-01T09:10:17-05:00 · **fix(terminal): keep pasted path chips on screen**
- **[fix]** `7f515a9` · 2026-09-01T07:43:11-05:00 · **fix(terminal): keep plain clicks input-free**
- **[fix]** `3e5a5a7` · 2026-09-01T07:14:05-05:00 · **fix(main): hide window while terminals keep running**
- **[speed]** `78ecdb7` · 2026-08-31T22:57:56-05:00 · **perf: bound history and async hot paths**
- **[fix]** `5e62101` · 2026-08-31T22:37:17-05:00 · **fix(dev): load supervised Vite renderer**
- **[internal]** `216d8fe` · 2026-08-31T22:13:37-05:00 · **test(release): isolate packaged provider probes**
- **[fix]** `20d2ff4` · 2026-08-31T22:13:35-05:00 · **fix(terminal): make workspace splitters draggable**
- **[internal]** `4babfbc` · 2026-08-31T22:11:18-05:00 · **test(terminal): lock Grok pass-off path**
- **[record]** `a6b7bea` · 2026-08-31T21:57:28-05:00 · **docs(agents): gate production staging on owner approval**
- **[record]** `ddbf61c` · 2026-08-31T19:59:09-05:00 · **docs(agents): Dev app checkout lives outside the repo so isolated panes can restart it**
- **[look]** `b9873d3` · 2026-08-31T19:52:09-05:00 · **style(terminal): lift the pane tab strip off the terminal canvas**
- **[look]** `c1c3e77` · 2026-08-31T19:45:55-05:00 · **style(workspace): lift toolbar off the terminal canvas**
- **[fix]** `157cd79` · 2026-08-31T19:39:13-05:00 · **fix(terminal): use a pane-layout icon for Tidy**
- **[internal]** `85705ff` · 2026-08-31T19:09:01-05:00 · **chore(release): bump Dev build to 0.3.39**
- **[internal]** `fd32f80` · 2026-08-31T18:57:38-05:00 · **test(smoke): snapshot pane styles before reflow**
- **[internal]** `39d67a3` · 2026-08-31T18:39:06-05:00 · **chore(release): bump Dev build to 0.3.38**
- **[feature]** `dc938d8` · 2026-08-31T18:38:26-05:00 · **feat(terminal): add toolbar icons and remove overview action**
- **[fix]** `effc691` · 2026-08-31T18:37:20-05:00 · **fix(terminal): reflow panes after removal**
- **[feature]** `72f80c2` · 2026-08-31T16:38:52-05:00 · **feat(terminal): Pass Off — version 0.3.32, codemap invariant updated**
- **[internal]** `ad74c40` · 2026-08-31T16:31:04-05:00 · **test(terminal): pin the handoff spread, the shell filter and the disabled gate**
- **[feature]** `b05051f` · 2026-08-31T16:21:37-05:00 · **feat(terminal): Pass off to… hands a pane's work to a new agent**
- **[fix]** `a8158ad` · 2026-08-31T16:07:53-05:00 · **fix(renderer): budget the pass-off transcript in UTF-8 bytes**
- **[feature]** `0d991fd` · 2026-08-31T15:30:07-05:00 · **feat(renderer): pass-off transcript shaping and xterm readLines**
- **[internal]** `520c337` · 2026-08-31T15:25:20-05:00 · **test(host): pass-off createTerminal round-trip**
- **[fix]** `5b1ccd4` · 2026-08-31T15:12:28-05:00 · **fix(terminal): a pass-off pane keeps the approved root that owns it**
- **[feature]** `5abd5be` · 2026-08-31T14:53:56-05:00 · **feat(terminal): pass-off creates spawn in the source's live cwd with a transcript prompt**
- **[internal]** `0453588` · 2026-08-31T14:36:15-05:00 · **test(smoke): packaged Claude exits now assert the shell handoff**
- **[record]** `21151e8` · 2026-08-31T14:31:12-05:00 · **docs(terminal): record that a pass-off prompt binds after the chat**
- **[internal]** `c290601` · 2026-08-31T14:28:23-05:00 · **chore(release): bump to 0.3.35 (exit-to-shell handoff over screenshot-drop fix)**
- **[internal]** `e7245de` · 2026-08-31T14:25:36-05:00 · **chore(release): bump to 0.3.34 (exit-to-shell pane handoff)**
- **[feature]** `cb5b13a` · 2026-08-31T14:25:36-05:00 · **feat(terminal): an agent CLI exiting cleanly hands its pane to a shell in its cwd**
- **[feature]** `884a4e5` · 2026-08-31T14:18:38-05:00 · **feat(terminal): bind a pass-off prompt into a launch argv**
- **[fix]** `8af2665` · 2026-08-31T14:11:56-05:00 · **fix(terminal): reap the abandoned lsof off the calling thread**
- **[fix]** `4962879` · 2026-08-31T14:07:43-05:00 · **fix(terminal): bound the live-cwd lsof wait**
- **[feature]** `ff3af8b` · 2026-08-31T14:00:19-05:00 · **feat(terminal): read a spawned child's live cwd on macOS**
- **[record]** `e21ef4e` · 2026-08-31T12:48:12-05:00 · **docs: pass-off live cwd via lsof (workspace forbids unsafe)**
- **[feature]** `777cc2b` · 2026-08-31T11:23:56-05:00 · **feat(terminal): pass-off cwd rule accepts directories inside an approved root**
- **[feature]** `75587ab` · 2026-08-31T11:16:52-05:00 · **feat(contracts): TerminalHandoffV1 on createTerminal**
- **[internal]** `9297c2f` · 2026-08-31T11:16:49-05:00 · **chore(release): bump to 0.3.34 (volatile screenshot drops)**
- **[record]** `76f2b44` · 2026-08-31T11:13:15-05:00 · **docs(agents): land on dev without checking it out; dev is pinned to .worktrees/dev**
- **[record]** `909309c` · 2026-08-31T11:12:11-05:00 · **docs: terminal Pass Off implementation plan**
- **[internal]** `6e8fe6a` · 2026-08-31T11:10:53-05:00 · **chore(release): bump to 0.3.33 (zombie process-group teardown)**
- **[fix]** `c81069a` · 2026-08-31T11:10:34-05:00 · **fix(terminal): treat zombie-only process groups as absent during teardown**
- **[fix]** `b8dd85a` · 2026-08-31T11:08:16-05:00 · **fix(renderer): store volatile-path screenshot drops by bytes**
- **[record]** `4f48a5d` · 2026-08-31T11:05:32-05:00 · **docs: terminal Pass Off design spec**
- **[record]** `7d9c1e7` · 2026-08-31T10:51:51-05:00 · **docs(agents): closeout pushes to dev, restarts the Dev app, then stages**
- **[feature]** `304a32e` · 2026-08-31T10:20:23-05:00 · **feat(terminal): pane header reads dot · title · ⋯ ⤢ + ×, with a working ticker**
- **[internal]** `d0247ef` · 2026-08-31T10:16:49-05:00 · **chore(release): bump to 0.3.32 (codex resume collision)**
- **[fix]** `2e8968f` · 2026-08-31T10:16:49-05:00 · **fix(terminal): codex reopen drops --last when a live pane shares the cwd**
- **[internal]** `3c19b8e` · 2026-08-31T10:06:40-05:00 · **chore(release): bump to 0.3.31 (native worktree panes)**
- **[feature]** `0953864` · 2026-08-31T09:38:59-05:00 · **feat(terminal): give fresh Claude and Grok panes their own git worktree**
- **[feature]** `2f63431` · 2026-08-31T08:41:19-05:00 · **feat: preserve background agents and refine terminal input**
- **[fix]** `4b5c420` · 2026-08-30T23:31:46-05:00 · **fix(terminal): keep the focused pane ring inside the pane**
- **[internal]** `ef63408` · 2026-08-30T22:59:57-05:00 · **chore(release): bump Dev build to 0.3.28**
- **[fix]** `5cc7546` · 2026-08-30T22:59:35-05:00 · **fix(smoke): verify the rendered final TUI frame**
- **[fix]** `750fcb2` · 2026-08-30T22:54:03-05:00 · **fix(smoke): elect the truncated-agent without a session-id flag**
- **[fix]** `1c817c9` · 2026-08-30T22:48:14-05:00 · **fix(smoke): claim redraw target in the PTY cwd**
- **[fix]** `c8f4b56` · 2026-08-30T22:41:57-05:00 · **fix(smoke): reserve redraw target for terminal sessions**
- **[fix]** `fcda581` · 2026-08-30T22:40:41-05:00 · **fix(smoke): claim the truncated-agent PTY without host env inheritance**
- **[internal]** `af55e94` · 2026-08-30T22:34:31-05:00 · **test: prove truncated-agent redraw in packaged smoke**
- **[fix]** `726c1c9` · 2026-08-30T22:31:35-05:00 · **fix(terminal): draw a 1px focused pane ring above the xterm canvas**
- **[feature]** `7a6276d` · 2026-08-30T22:00:06-05:00 · **feat(electron): mark Interface Dev with canary chip and Dock icon**
- **[record]** `17cc4d5` · 2026-08-30T20:29:16-05:00 · **docs: close out every finished task by pushing and staging on dev**
- **[internal]** `6002888` · 2026-08-30T20:13:25-05:00 · **test: accept inset focused terminal rings in packaged smoke**
- **[fix]** `0c09f79` · 2026-08-30T20:08:41-05:00 · **fix(ui): add depth to Chat, Code, and Agent selectors**
- **[fix]** `3a72a89` · 2026-08-30T20:04:52-05:00 · **fix(ui): blue CLI caret and inset terminal focus ring**
- **[internal]** `45874b4` · 2026-08-30T19:52:46-05:00 · **chore(release): 0.3.23 efficiency review and Project Agent removal**
- **[feature]** `1f4f9cf` · 2026-08-30T18:21:25-05:00 · **dev: run the dev instance beside the installed app with live restarts**
- **[fix]** `6d5f66b` · 2026-08-30T17:12:36-05:00 · **fix(main): keep update restart from stalling on a main-process error**
- **[feature]** `2730565` · 2026-08-30T16:38:49-05:00 · **Project-agent canonical intake checkpoint**
- **[internal]** `24190b6` · 2026-08-30T16:21:39-05:00 · **chore(release): bump to 0.3.21**
- **[feature]** `2e20a37` · 2026-08-30T16:14:36-05:00 · **feat(terminal): Flip toggles panes to title cards**
- **[feature]** `6437c19` · 2026-08-30T15:40:17-05:00 · **Project-agent canonical intake checkpoint**
- **[feature]** `43bcb35` · 2026-08-30T10:29:39-05:00 · **Project-agent canonical intake checkpoint**
- **[feature]** `2b4363f` · 2026-08-30T09:42:55-05:00 · **Project-agent canonical intake checkpoint**
- **[feature]** `69091f8` · 2026-08-30T09:28:11-05:00 · **Project-agent canonical intake checkpoint**
- **[feature]** `d1d69b9` · 2026-08-29T19:44:22-05:00 · **feat: unify terminal-first Code workspace**
- **[feature]** `f5e1e6e` · 2026-08-29T17:57:31-05:00 · **feat: port TypeScript delta to Rust Interface**
- **[feature]** `3278559` · 2026-08-29T10:01:47-05:00 · **Full review reduction pass, owner-ruled fixes, and agent codemap**
- **[feature]** `105b985` · 2026-08-28T14:42:22-05:00 · **feat: The Fold app icon, white on emerald**

## All Dev Landings

### Scrub personal project names from test and preview fixtures

- **Completed:** 2026-11-24T17:19:56-06:00
- **Kind:** internal
- **Release:** v0.3.182
- **Commit:** `53513d2` (`docs/scrub-public-fixtures-20261124`)
- **Notes:** Scrub personal project names from test and preview fixtures
- **Supersedes:** 6c20593 — only the sample project name in the test changed, so the published source carries no personal project name; behaviour is unchanged; 21180fa — only the sample project names in the fixture and test changed, so the published source carries no personal project names; behaviour is unchanged

### No false Codex error without Codex; stuck terminals recover while a VM or Internet Sharing is on

- **Completed:** 2026-11-24T09:36:16-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `268ff7c` (`fix/codex-absent-listing`)
- **Notes:** On a Mac without Codex the first screen shows no Codex error; Dev still lists your Codex sessions as before.
- **Commits:** Codex not installed is not a failed refresh; a nobody-owned process no longer fails orphan recovery
- **Pins:** `crates/engine-codex/src/adapter_tests.rs::listing_with_no_codex_installed_is_unavailable_not_a_failed_refresh`, `crates/interface-terminal/src/embedded/orphan.rs::a_row_owned_by_nobody_does_not_fail_the_listing`

### Public source: README for strangers, MIT licence, one-line installer, export tool

- **Completed:** 2026-11-23T18:38:01-06:00
- **Kind:** internal
- **Release:** v0.3.182
- **Commit:** `b1fa93d` (`docs/public-source-20261123`)
- **Notes:** Record the pinned test whose sample path changed
- **Supersedes:** bdb3d46 — only the sample home folder in the test changed, so the published source carries no personal path; behaviour is unchanged

### Update rustls-webpki and url past their published advisories

- **Completed:** 2026-11-23T18:34:54-06:00
- **Kind:** internal
- **Release:** v0.3.182
- **Commit:** `37fdc6e` (`fix/dependency-advisories-20261123`)
- **Notes:** Update rustls-webpki and url past their published advisories

### Browser pane: safe popup windows, quarantined downloads, ask before no-click downloads

- **Completed:** 2026-11-23T18:32:01-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `7ee05f5` (`fix/browser-pane-popups-downloads-20261123`)
- **Notes:** In a Browser tab a site's popup always has a title bar and close button; its downloads get the macOS internet-download check; self-started ones ask.
- **Commits:** Browser pane: popups are ordinary windows; downloads are quarantined and ask without a click; Browser pane: reword four comments the packaging test read as imports
- **Pins:** `electron/src/main/browser-pane.test.ts::builds a page's popup window itself: framed, closable, movable, opaque, focusable and never on top`, `electron/src/main/browser-pane.test.ts::keeps a popup window on its screen, no larger than it and never too small to show its site`, `electron/src/main/browser-pane.test.ts::flags a finished download for Gatekeeper before calling it saved, and fails and removes one it cannot flag`, `electron/src/main/browser-pane.test.ts::asks before saving a download the page started without a click, and never for the owner's own`

### Team tools answer only their own thread's agent process

- **Completed:** 2026-11-23T18:28:53-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `e0b22d4` (`fix/team-socket-caller-20261123`)
- **Notes:** Nothing looks different; agents in threads still start and read other threads, but another agent's command can no longer use a thread's tools.
- **Commits:** Team tools sockets answer only their own session's provider process
- **Pins:** `apps/interface-host/src/threads_team_tests.rs::a_threads_team_tools_answer_only_its_own_provider_and_the_processes_it_started`, `apps/interface-host/src/team_tools.rs::a_team_socket_answers_only_its_own_provider_and_the_processes_under_it`, `crates/interface-terminal/src/proc_ancestry.rs::a_read_listing_finds_this_test_process_above_a_child_it_spawned`

### Interface's own git calls never run programs a project's .git settings name

- **Completed:** 2026-11-23T18:25:42-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `2221a26` (`fix/filter-drivers-20261123`)
- **Notes:** Nothing looks different; a downloaded project whose .git names its own filter or ssh program no longer runs it on Review or a checkpoint.
- **Pins:** `crates/interface-review/tests/suite/repository_programs.rs::review_never_runs_a_filter_the_repository_defines`, `crates/interface-review/tests/suite/repository_programs.rs::thread_checkpoint_never_runs_a_filter_the_repository_defines`, `crates/interface-review/tests/suite/repository_programs.rs::a_filter_from_a_file_the_repository_settings_include_never_runs`

### Sign builds hardened, timestamped and team-pinned so installed copies keep updating

- **Completed:** 2026-11-23T18:22:20-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `6444b00` (`fix/public-signing-and-updates-20261123`)
- **Notes:** The next promoted build arrives through Restart to update as usual; in that build Settings > Updates shows updates on, not off.
- **Commits:** Read codesign's own "/* exists */" in the running app's requirement; Sign every build hardened, timestamped and pinned to the Apple team; Refuse to package or publish a build whose updates would be off; List the release channel file as an input of the Electron suite; Record the pinned signing test this branch replaces
- **Pins:** `crates/interface-updater/src/signature.rs::codesign_requirements_of_real_builds_are_read_with_exists_spelled_out`, `xtask/src/package.rs::an_app_without_the_hardened_runtime_timestamp_and_team_is_refused`, `electron/scripts/sign-mac.test.cjs::electron-osx-sign emits the exact hardened, timestamped and team-pinned arguments`, `electron/scripts/smoke-packaged.test.mjs::a packaged build with a public update channel must come up with updates on`
- **Supersedes:** 3b37ef46 — Apple Development builds are now hardened and timestamped; the untimestamped local path is gone

### Record the launch review's third pass: privacy page, ask-first defaults, folder trust

- **Completed:** 2026-11-21T12:54:24-06:00
- **Kind:** record
- **Release:** v0.3.182
- **Commit:** `473eed8` (`docs/launch-review-record-20261121`)
- **Notes:** Record the launch review's third pass: privacy page, ask-first defaults, folder trust

### Trust this folder? is asked once; until trusted, Claude runs without the folder's own settings

- **Completed:** 2026-11-21T12:53:03-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `a1fb62d` (`feat/folder-trust-ui-20261121`)
- **Notes:** Start a Claude thread in a folder never opened in Claude Code: 'Trust this folder?' appears before the first message is sent
- **Commits:** Folder trust, core: an untrusted folder's own settings never load into a Claude session; Folder trust: ask once per folder; until trusted, Claude threads run on the owner's own settings (protocol 72); Folder trust: a repository cloned into an agent's home is not trusted through the home
- **Pins:** `crates/engine-claude/src/adapter_tests.rs::an_untrusted_folders_own_settings_never_load`, `crates/interface-settings/src/folder_trust.rs::a_folder_cannot_claim_to_be_a_worktree_of_a_repository_it_was_never_added_to`, `crates/interface-desktop-services/src/service/folder_trust_tests.rs::a_repository_cloned_into_an_agents_home_is_not_trusted_through_the_home`, `apps/interface-host/tests/suite/folder_trust.rs::an_untrusted_folders_thread_runs_on_user_settings_until_trusted_then_resumes_with_its_own`, `electron/src/renderer/folder-trust.test.ts::runs the whole question before a new thread's first prompt, answer first`

### Agents ask before a web search or acting on another pane; each has a switch to allow it

- **Completed:** 2026-11-21T12:36:55-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `0477fca` (`fix/ask-first-defaults-20261121`)
- **Notes:** Settings > Threads shows 'Pane tools without asking', off. An agent's work settings show 'Search the web without asking', off
- **Commits:** Agents ask before web searches and before pane tools that act on other panes; owner switches for both (protocol 71)
- **Pins:** `crates/interface-desktop-services/src/agent_policy.rs::a_teammates_web_search_asks_unless_its_owner_switched_searching_on`, `crates/interface-desktop-services/src/agent_policy.rs::the_web_search_switch_does_not_loosen_web_fetches_or_anything_else`, `apps/interface-host/src/bootstrap_tests.rs::pane_tools_that_act_on_other_panes_ask_unless_the_owner_switched_them_on`, `crates/interface-settings/src/threads_tests.rs::the_pane_tools_switch_lives_beside_the_store_and_anything_but_on_means_ask`
- **Supersedes:** e3730f55 — web search now asks by default (owner, 2026-11-21); the test's last assertion changes from allowed to asked; a83ebb34 — Codex pane tools that act on other panes now ask by default (owner, 2026-11-21); the pinned argv's iface tools line changes; 497156ad — host protocol 71; the outside agents suite test only asserts the version number; c79f64e2 — host protocol 71; the project Away suite test only asserts the version number

### Settings gets a Privacy page; a one-time privacy summary; a note beside switching accounts

- **Completed:** 2026-11-21T12:24:00-06:00
- **Kind:** feature
- **Release:** v0.3.182
- **Commit:** `55a1272` (`feat/privacy-page-20261121`)
- **Notes:** Open Settings: Privacy sits after Updates and lists what leaves this Mac. A small card bottom-right says the same, once
- **Commits:** Settings › Privacy, a once-only privacy summary card, and an account-terms note
- **Pins:** `electron/src/renderer/privacy.test.ts::says what leaves this Mac in the owner's words`, `electron/src/renderer/privacy.test.ts::shows once per profile, until either answer`, `electron/src/renderer/privacy.test.ts::is a corner status card with Open Privacy and Got it, never a dialog`, `electron/src/renderer/privacy.test.ts::goes under a thread's limit line only when it offers another account`

### Update channel points at GitHub releases; nothing published yet reads as up to date

- **Completed:** 2026-11-21T12:03:11-06:00
- **Kind:** feature
- **Release:** v0.3.182
- **Commit:** `5c2f97d` (`feat/update-channel-github`)
- **Notes:** Settings > Updates > Check for updates still says up to date; nothing is published on the public channel yet
- **Commits:** Update channel points at GitHub releases with two keys; an empty feed reads as up to date
- **Pins:** `crates/interface-updater/src/remote.rs::a_feed_with_nothing_published_yet_is_not_a_fault`, `crates/interface-updater/src/service.rs::a_feed_with_nothing_published_yet_reads_up_to_date_even_when_asked`, `crates/interface-updater/src/remote.rs::the_shipped_client_reads_a_missing_manifest_as_nothing_published`

### Pull-request watch ignores strangers; dropped text is pasted; app offers to move to Applications

- **Completed:** 2026-11-21T12:00:33-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `4c8bcad` (`fix/launch-review-batch1`)
- **Notes:** Drag text from a web page onto a terminal: it shows at the prompt and waits for Return. Nothing else looks different
- **Commits:** Launch-review fixes: a watch hears only project reviewers; dropped text is pasted; Move to Applications
- **Pins:** `crates/interface-review/src/thread_ship_tests.rs::a_watch_hears_only_the_projects_own_reviewers`, `apps/interface-host/src/threads_schedules_tests.rs::a_strangers_review_comment_never_reaches_a_watched_thread`, `electron/src/renderer/terminal-drop.test.ts::drops every other control character, so a drag cannot press keys of its own`, `electron/src/renderer/terminal-drop.test.ts::reaches the program as a paste, never as typed input`, `electron/src/main/applications-folder.test.ts::is made to a packaged app opened from anywhere else on a Mac`

### Public update channel, switched off: signed releases reach installed copies with no re-download

- **Completed:** 2026-11-21T11:39:03-06:00
- **Kind:** feature
- **Release:** v0.3.182
- **Commit:** `8ec6042` (`feat/public-update-channel`)
- **Notes:** Nothing changes on screen yet: Settings > Updates works as before; the public channel stays off until its address and keys are set
- **Commits:** Public update channel: signed releases reach installed copies without a re-download
- **Pins:** `crates/interface-updater/src/remote.rs::a_newer_signed_release_lands_in_the_local_feed_byte_for_byte`, `crates/interface-updater/src/remote.rs::an_archive_that_is_not_the_signed_bytes_never_reaches_the_installer`, `crates/interface-updater/src/remote.rs::an_older_release_offered_again_is_refused_once_a_newer_one_was_seen`, `crates/interface-updater/src/signed_manifest.rs::a_manifest_signed_by_anyone_else_is_refused`, `crates/interface-updater/src/service.rs::a_published_release_is_downloaded_checked_and_handed_to_the_installer`, `apps/interface-host/src/updater.rs::a_build_with_a_public_channel_ignores_a_feed_from_the_environment`

### Phone access stays reachable when someone floods the Mac's phone listener or relay

- **Completed:** 2026-11-17T21:28:47-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `75ca85f` (`fix/phone-connection-limits`)
- **Notes:** With phone access on, the iPhone still connects nearby and through the relay; nothing looks different
- **Commits:** Phone access stays reachable when someone floods the Mac's phone listener or the relay
- **Pins:** `crates/interface-remote/src/http/direct_tests.rs::one_address_holding_slow_connections_never_shuts_a_phone_out`, `crates/interface-remote/src/http/direct_tests.rs::a_trickled_request_head_loses_its_connection_at_the_deadline`, `crates/interface-remote/src/http/direct_tests.rs::a_phone_on_a_weak_network_is_still_answered`, `crates/interface-remote/src/http/direct_tests.rs::strangers_holding_relay_sockets_never_keep_the_mac_out`, `crates/interface-remote/src/http/direct_tests.rs::slow_uploads_to_the_relay_never_keep_the_mac_or_a_phone_out`, `crates/interface-remote/src/http/direct_tests.rs::the_mac_comes_back_after_its_relay_restarts`, `crates/interface-remote/src/relay_admission.rs::a_full_address_displaces_only_its_own_oldest_stranger`

### Harden approval gates, voice redaction, browser downloads and updater checks

- **Completed:** 2026-11-17T10:10:51-06:00
- **Kind:** fix
- **Release:** v0.3.182
- **Commit:** `94fa1c4` (`fix/security-audit-hardening`)
- **Notes:** Voice mode hides passwords, and untrusted repos, downloads and updates run stricter safety checks
- **Commits:** Harden Interface defensive security boundaries across Electron and Rust
- **Pins:** `electron/src/renderer/voice-ui.test.ts::omits password input values and data-voice-redact text and values from snapshots`, `crates/engine-claude/src/adapter_tests.rs::auto_mode_escalations_and_init_fallback_ask_the_user`, `crates/interface-updater/src/service.rs::retained_archive_reverify_refuses_tampered_install`

### Record the 0.3.181 release

- **Completed:** 2026-11-16T11:16:36-06:00
- **Kind:** record
- **Release:** v0.3.182
- **Commit:** `a0b89a4` (`record-0-3-181`)
- **Notes:** Record the 0.3.181 release

### Live backgrounds: the sidebar is frosted glass

- **Completed:** 2026-11-16T11:05:37-06:00
- **Kind:** look
- **Release:** v0.3.181
- **Commit:** `71cedd9` (`look/frosted-sidebar-20261116`)
- **Notes:** Pick a live background in Dev: the sidebar shows soft blurred colour, with no lines or shapes behind the workspace names
- **Pins:** `electron/src/renderer/theme-look.test.ts::frosts the sidebar from a still copy of the picture, not by blurring behind it on every frame`, `electron/src/renderer/theme-look.test.ts::leaves the sidebar's frosted copy still between refreshes, then brings it up to the picture`
- **Supersedes:** 6364daab — the sidebar is now always frosted over a live background, so one blur is written without the theme file asking for it; it sits on a still copy of the art, not on anything that moves

### Threads take the TN Code look: tiles, bubble with a tail, stamps, round composer

- **Completed:** 2026-11-16T10:38:19-06:00
- **Kind:** look
- **Release:** v0.3.181
- **Commit:** `10548fe` (`feat/threads-tn-look`)
- **Notes:** Open Threads: rows lead with an initials tile, your prompt is a bubble with a tail under a timestamp, and the composer is a rounded card with a round Send
- **Pins:** `electron/src/renderer/thread-transcript.test.ts::stamps the first timed prompt, then one an hour or more after the last stamp`, `electron/src/renderer/thread-inbox.test.ts::shows the first letters of the title's first two words, in capitals`, `electron/src/renderer/components/ThreadComposer.test.ts::sits on the transcript's reading lane as the canvas's card`
- **Supersedes:** 86247089 — the owner replaced the Claude-app Threads look with the TN Code look (2026-11-16): the hue is the row's tile, the bubble and card are worked out from theme tokens; 48ff6fd9 — the TN Code look (owner, 2026-11-16): a full row's line two no longer leads with the hue square, which is the tile now

### Record the pane shortcuts and Rex-style fade landings and their open owner checks

- **Completed:** 2026-11-15T22:35:01-06:00
- **Kind:** record
- **Release:** v0.3.181
- **Commit:** `6cb7f7f` (`todo-cmd-w-checks`)
- **Notes:** Record the pane shortcuts and Rex-style fade landings and their open owner checks

### Record the 0.3.179 and 0.3.180 releases and the automatic thread titles

- **Completed:** 2026-11-15T22:34:23-06:00
- **Kind:** record
- **Release:** v0.3.181
- **Commit:** `a070891` (`docs/release-0180-record-20261115`)
- **Notes:** Record the 0.3.179 and 0.3.180 releases and the automatic thread titles

### Live backgrounds: panes you are not in fade, the way Rex does it

- **Completed:** 2026-11-15T22:33:48-06:00
- **Kind:** look
- **Release:** v0.3.181
- **Commit:** `b462306` (`pane-fade-inactive-looks`)
- **Notes:** Pick a live background (Orbit) with two or more panes: the selected pane is at full strength and the others are faded; click another pane and the fade follows.
- **Pins:** `electron/src/renderer/theme-look.test.ts::fades every pane but the selected one, the way Rex does, and never a pane that is waiting`

### ⌘↩ opens a terminal pane and ⌘W closes the selected pane instead of the window

- **Completed:** 2026-11-15T22:28:55-06:00
- **Kind:** feature
- **Release:** v0.3.180
- **Commit:** `6799ba3` (`pane-focus-contrast-shortcuts`)
- **Notes:** In Code, press ⌘↩ in a pane: a new shell opens beside it. Press ⌘W: the selected pane closes and the window stays open. ⇧⌘W closes the window.
- **Commits:** ⌘↩ opens a terminal pane; ⌘W closes the selected pane, not the window
- **Pins:** `electron/src/renderer/terminal-keys.test.ts::opens a pane with ⌘↩ and leaves every other Enter to the program`, `electron/src/renderer/workspace.test.ts::⌘W closes the selected pane, never the window`, `electron/src/main/app-menu.test.ts::⌘W is Close Pane and Close Window moves to ⇧⌘W`

### Threads are named by Claude Haiku 5.5 from the first message, on by default

- **Completed:** 2026-11-15T22:25:30-06:00
- **Kind:** feature
- **Release:** v0.3.180
- **Commit:** `ede3a86` (`feat/haiku-thread-titles-20261115`)
- **Notes:** Send a new thread's first message: within a second or two its title becomes a short generated one, in the pane head and the list, while the agent still works
- **Pins:** `apps/interface-host/src/threads_tests.rs::a_threads_title_is_asked_for_as_its_first_prompt_goes_out_without_waiting_for_its_run`, `crates/interface-settings/src/threads_tests.rs::generated_titles_are_turned_on_once_and_an_owners_later_off_stays_off`, `crates/engine-claude/src/title_tests.rs::a_title_is_named_from_the_first_prompt_alone`

### Terminal text matches Rex: MonoLisa Bold at 11.5, same rows and columns

- **Completed:** 2026-11-15T22:18:42-06:00
- **Kind:** look
- **Release:** v0.3.180
- **Commit:** `b205155` (`feat/terminal-rex-type`)
- **Notes:** Open a terminal pane: the text is MonoLisa Bold, smaller and tighter like a Rex pane, and Settings shows Terminal text size 11.5 px
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::uses the owner's terminal face, MonoLisa Bold, instead of the app chrome face`, `electron/src/renderer/terminal-cell.test.ts::matches the owner's Rex pane: MonoLisa 11.5px is 15 device pixels wide on Retina`, `crates/interface-desktop-services/src/service_tests.rs::workspace_presentation_v6_keeps_half_sizes_and_older_surfaces_read_whole_ones`
- **Supersedes:** cf6dfa33 — rows are the font's own height again, to match the owner's Rex terminal (owner, 2026-11-15); c79f64e2 — host protocol 70 (terminal text in half steps); the project Away suite test only asserts the version number; 497156a — host protocol 70 (terminal text in half steps); the outside agents suite test only asserts the version number

### Pane headers show only the name, status dot and icons; Find, Files, folder and status words are gone

- **Completed:** 2026-11-15T22:03:07-06:00
- **Kind:** look
- **Release:** v0.3.180
- **Commit:** `a948804` (`look/pane-head-dot-only-20261115`)
- **Notes:** Every terminal pane's top bar shows only its name, the small status dot and the icons on the right; no Find, Files, folder name, Idle or Working · time
- **Commits:** Code: a pane's head is its tabs, status dot and icons; the folder, state words, Find and Files are gone
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::draws no folder, no state in words, no Find and no Files: a tab's dot is its state`
- **Supersedes:** 677f7c5a5d11f8cc38641b43c25ad0616089a5f7 — the owner asked (2026-11-15) for the folder, state words, Find and Files it pinned in the pane head to be removed; the test now pins that the head draws none of them

### Record the UI redesign landings from the design canvas

- **Completed:** 2026-11-15T21:37:12-06:00
- **Kind:** record
- **Release:** v0.3.179
- **Commit:** `751ae8f` (`docs/ui-redesign-record-20261115`)
- **Notes:** Record the UI redesign landings from the design canvas

### Settings fills the window: one row style, plain words, Live backgrounds first

- **Completed:** 2026-11-15T21:36:30-06:00
- **Kind:** look
- **Release:** v0.3.179
- **Commit:** `911a77b` (`ui/settings-page-20261115`)
- **Notes:** Open Settings: it takes the whole window with its own menu; Appearance starts with Live backgrounds and a Motion menu; Updates shows What's new in plain words
- **Commits:** Settings fills the window, with its own menu where the sidebar was; Settings: one row pattern, one switch, and Appearance and Threads as drawn; Models & providers: what needs setup first, then what is ready, then More; Updates: one card for the state you are in, and What's new in plain sentences; Settings: Agent customization, iPhone access and Shortcuts in the same rows; Settings: menu and tab selection read in every theme; providers never wrap; Record where the new Settings look lives and how it is checked; Settings: the page column stays in place between pages that scroll and pages that do not; Record the owner's approval of the see-through sliders' new row in Settings
- **Pins:** `electron/src/renderer/components/SettingsPage.test.ts::ends the menu with the version and notes the pages that want you`, `electron/src/renderer/components/SettingsPage.test.ts::sets how live backgrounds move with one Motion menu`, `electron/src/renderer/components/SettingsPage.test.ts::opens each provider's accounts from its own Accounts menu`, `electron/src/renderer/components/SettingsPage.test.ts::leads the picker with the theme files that move, under Live backgrounds`
- **Supersedes:** 2a980803 — Updates no longer shows the current/available/last-checked grid or opens the full log by default; the test checks the plain status card and the log behind Developer log; adf55c79 — the Follow macOS switch no longer has the words "Follow macOS" beside it; the test finds it by its row's name instead

### Pane headers say name, folder and status in words; Find and Files move into the header

- **Completed:** 2026-11-15T21:26:46-06:00
- **Kind:** look
- **Release:** v0.3.179
- **Commit:** `c2d8594` (`ui/pane-header-20261115`)
- **Notes:** A terminal pane's head reads like 'Claude Code · home · Working · 2 min', with Find and Files beside the ⋯ menu; nothing floats over the terminal text
- **Commits:** Code: each pane's head says its folder and state, and nothing floats over the terminal's text
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::names the pane, its folder and its state in words, then Find and Files before the icons`, `electron/src/renderer/components/TerminalPrimitives.test.ts::draws nothing over the text: search, copy and paste wait for the pane head's request`
- **Supersedes:** b0fae24786e6aa053faa521aa88d4b2f1c34ff9d — the floating terminal toolbar it pinned was removed at the owner's request (2026-11-15); the test now pins that nothing floats over the text; 784e59702b615a26ed6e11c5707854cd0dc5c699 — the floating terminal toolbar it pinned was removed at the owner's request (2026-11-15); the test now pins that nothing floats over the text

### Sidebar footer is one quiet row, and errors are small notices in the corner

- **Completed:** 2026-11-15T21:22:07-06:00
- **Kind:** look
- **Release:** v0.3.179
- **Commit:** `90fc6cd` (`ui/shell-footer-notices-20261115`)
- **Notes:** The sidebar's foot reads Settings with a small amber 'providers need setup'; an error shows as a small card bottom-right with Try again and Dismiss
- **Commits:** Sidebar: Settings and "providers need setup" share one quiet footer row; Errors become small notices in the bottom-right corner instead of a bar
- **Pins:** `electron/src/renderer/components/Sidebar.test.ts::puts Settings and the setup notice in one quiet footer row, without the version`, `electron/src/renderer/corner-notices.test.ts::names the provider by its display name in one sentence`

### Threads: a short title over one plain line, and a kept draft inside the composer

- **Completed:** 2026-11-15T21:17:38-06:00
- **Kind:** look
- **Release:** v0.3.179
- **Commit:** `dc43b9a` (`ui/thread-view-20261115`)
- **Notes:** Open a thread: its title is one short line, then project · branch · where edits land. A message that was not sent shows inside the composer
- **Commits:** Threads: a kept message sits on the composer's top edge as one line; Threads: a short title over one plain line, and a whole focus ring; Threads: the kept-message line shows only when there is something to recover
- **Pins:** `electron/src/renderer/thread-title.test.ts::takes the prompt's first line, collapses its white space and cuts it on a word near 60 characters`, `electron/src/renderer/components/PromptRecoveryPanel.test.ts::shows only a message with something to recover, never one delivered and answered`
- **Supersedes:** 9fe5ab596f1b1582c74e4bf9d534b7b04577372b — AgentTile.test.ts "reads as a breadcrumb: the project, then the title" now asserts the owner-approved two-line head (title, then project and where it works)

### Record the Live backgrounds landing and the release still waiting

- **Completed:** 2026-11-15T20:26:19-06:00
- **Kind:** record
- **Release:** v0.3.179
- **Commit:** `7d44917` (`docs/review-record-live-20261115`)
- **Notes:** Record the Live backgrounds landing and the release still waiting

### Settings: themes with a moving backdrop lead the theme picker as Live backgrounds

- **Completed:** 2026-11-15T20:25:43-06:00
- **Kind:** look
- **Release:** v0.3.179
- **Commit:** `1115be0` (`look/live-backgrounds-group-20261115`)
- **Notes:** Settings › Appearance: the theme picker starts with Live backgrounds (Aurora, Glyph, Neon Grid, Orbit, Paper Tide, TN Code); click one to turn it on
- **Commits:** Theme files that move lead the theme picker as Live backgrounds
- **Pins:** `electron/src/renderer/components/SettingsPage.test.ts::leads the picker with the theme files that move, under Live backgrounds`

### Record the review of the 14-15 November features

- **Completed:** 2026-11-15T18:25:10-06:00
- **Kind:** record
- **Release:** v0.3.179
- **Commit:** `045b9af` (`docs/review-record-20261115`)
- **Notes:** Record the review of the 14-15 November features

### A prompt fired at launch keeps the thread's history; visual replies cannot reach the network

- **Completed:** 2026-11-15T18:23:25-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `4033fe4` (`fix/sched-visuals-review-20261115`)
- **Notes:** Quit with a schedule due, reopen: the thread shows its earlier conversation and the new turn. A link clicked in a visual no longer blanks the card
- **Commits:** A link clicked inside a visual reply no longer blanks the card; A scheduled prompt waiting for its thread shows as Next, not behind later ones; A thread that receives a prompt before the window read it still shows its past; A visual reply can no longer reach the network through WebRTC
- **Pins:** `electron/src/renderer/history-late-read.test.ts::still reads its history once and shows it in order, the live turn once`, `electron/src/main/visual-network-fence.test.ts::turns off WebRTC's own UDP and sends every request, loopback too, to a proxy that leads nowhere`, `electron/src/main/visual-network-fence.test.ts::fences the window's own session before its first document loads`, `crates/interface-settings/src/thread_schedules_tests.rs::a_firing_waiting_for_its_thread_comes_first_in_the_view`

### Threads search finds old messages in long threads and runs 3x faster; a crashed turn reads stopped

- **Completed:** 2026-11-15T18:18:01-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `47c6e43` (`fix/journal-review-20261115`)
- **Notes:** Search Threads for a word from early in a long older thread: it is found, with the word shown. After a crash a thread no longer shows Working forever
- **Commits:** In messages rows start their excerpt at the match, so the matching text shows; A thread cut off by a host crash reads as stopped after the restart, not Working forever; Threads search reads about a third as long, and a newer search stops the older ones; Threads search finds a long conversation's oldest messages: the journal imports the whole history and rebuilds short imports once
- **Pins:** `apps/interface-host/src/threads_tests.rs::an_eight_hundred_item_history_is_imported_whole`, `apps/interface-host/src/threads/journal_tests.rs::a_journal_imported_short_is_rebuilt_once_from_the_whole_history`, `apps/interface-host/src/threads/journal_tests.rs::a_failed_rebuild_keeps_the_old_journal`, `crates/interface-settings/src/thread_journal/tests.rs::search_skips_unparsed_lines_without_changing_what_it_finds`, `apps/interface-host/src/bootstrap_tests.rs::a_session_left_working_by_a_host_crash_opens_idle_without_rewriting_the_cache`, `electron/src/renderer/thread-inbox.test.ts::starts an In messages excerpt just before the match, so the matching text shows on line two`

### The Browser test clicks a new tab only once it takes pointer input

- **Completed:** 2026-11-15T18:13:16-06:00
- **Kind:** internal
- **Release:** v0.3.179
- **Commit:** `ec11a15` (`fix/browser-lane-input-20261115`)
- **Notes:** The Browser lane clicks a page only once it takes pointer input

### Composer: a secret card goes when its agent is gone; stash never doubles or drops typed words

- **Completed:** 2026-11-15T17:39:41-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `77303e7` (`fix/composer-review-20261115`)
- **Notes:** Ask an agent for a secret, then press Stop: the card disappears and the thread can ask again. ⌘S while typing leaves each word in one place only
- **Commits:** Stash keeps only the words typed while Rust stored the draft; Restoring a stash keeps words typed while the restore was on its way; The thread composer no longer redraws on every streamed event; Hands-free fake Claude: Stop during a `secret` turn closes its team tools connection; A secret request is withdrawn when the agent that asked can no longer take the answer; A run's end withdraws only the secret request that run made, whichever session asked
- **Pins:** `apps/interface-host/src/threads_team_tests.rs::a_pending_secret_is_withdrawn_when_its_connection_closes`, `apps/interface-host/src/threads_team_tests.rs::a_pending_secret_is_withdrawn_when_its_turn_ends`, `apps/interface-host/src/threads_team_tests.rs::a_late_end_of_the_previous_turn_never_withdraws_the_next_turns_secret`, `electron/src/renderer/use-draft-stash.test.ts::keeps only the words typed while Rust stored the draft, so nothing is in two places`, `electron/src/renderer/use-draft-stash.test.ts::keeps the words typed while a restore was on its way, after the restored draft`

### After a provider switch, the Changes count and Whole thread diff cover the whole thread

- **Completed:** 2026-11-15T17:30:02-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `aa9a026` (`fix/switch-totals-20261115`)
- **Notes:** Switch a thread Claude → Grok → Claude: the pane head's Changes +N matches the Changes panel's Whole thread, which lists every file
- **Commits:** A switched thread's Changes count totals every provider it ran on (protocol 69); A switch to Grok measures the handoff turn its agent starts on; A thread switched back to its first provider shows its whole diff again
- **Pins:** `crates/interface-settings/src/threads/chain_tests.rs::a_switched_threads_view_carries_every_segments_line_totals`, `apps/interface-host/src/threads_switch_tests.rs::an_acp_launch_handoff_starts_on_a_checkpoint_taken_before_the_agent`, `apps/interface-host/src/threads_switch_tests.rs::a_thread_switched_back_is_diffed_from_its_earliest_checkpoint`, `electron/src/renderer/thread-inbox.test.ts::totals a switched thread's every segment in the head and the row, and an unswitched one as before`
- **Supersedes:** c79f64e2 — host protocol 69 (switched thread totals); the project Away suite test only asserts the version number; 497156a — host protocol 69 (switched thread totals); the outside agents suite test only asserts the version number

### Looks: a theme can no longer move behind another app; the play mark shows only on themes that move

- **Completed:** 2026-11-15T17:22:03-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `31a49c4` (`fix/looks-review-20261115`)
- **Notes:** Settings › Appearance › Yours: a theme that animates only its layers gets the play mark, a still scene does not; no backdrop moves while Interface is behind
- **Commits:** A theme file can no longer keep its backdrop moving behind another app; A theme whose shader cannot draw no longer keeps a timer running; The play mark means a theme moves: layers that animate get it, a still scene does not
- **Pins:** `electron/src/renderer/theme-look.test.ts::is held all the same while Interface is behind another app`, `electron/src/renderer/theme-look.test.ts::rests when its only motion is a shader that cannot draw`, `crates/interface-settings/src/custom_themes.rs::marks_a_theme_moving_when_its_backdrop_layers_animate`, `crates/interface-settings/src/custom_themes.rs::marks_a_scene_moving_only_when_something_in_it_moves`

### A thread shows the permission mode it really runs in, after a reload and for helper threads

- **Completed:** 2026-11-15T17:16:12-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `6dec0da` (`fix/thread-mode-20261115`)
- **Notes:** After a reload a thread's mode chip shows what the agent really runs in (helpers show Accept edits), and the first send no longer switches its mode
- **Commits:** A thread shows the permission mode it really runs in, after a reload and for threads the host started (protocol 68); Picking Manual for a thread the host runs in another mode changes it; Record the pinned suite tests whose protocol number moved to 68
- **Pins:** `apps/interface-host/tests/suite/session_management.rs::listings_and_the_cached_registry_carry_the_mode_the_host_keeps`, `electron/src/renderer/session-presentation.test.ts::shows the host's mode after a reload even when another mode is saved for the model`, `electron/src/renderer/session-presentation.test.ts::never applies a saved mode before the first send of a session the host has a mode for`, `electron/src/renderer/session-presentation.test.ts::lets the owner pick Manual for a session the host runs in another mode`
- **Supersedes:** c79f64e2 — host protocol 68 (thread permission mode); the project Away suite test only asserts the version number; 497156a — host protocol 68 (thread permission mode); the outside agents suite test only asserts the version number

### Browser: a page opens tabs or windows only after a click; new-tab downloads say where files went

- **Completed:** 2026-11-15T17:10:41-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `f50f7d9` (`fix/browser-review-20261115`)
- **Notes:** A page can no longer open tabs or windows by itself; a popup's title starts with its site. A Download link that opens a tab says Saved to Downloads
- **Commits:** A download link that opens a new Browser tab says where the file went; A Browser page opens a tab or window only just after a press in it
- **Pins:** `electron/src/main/browser-pane.test.ts::refuses a window.open with no press, and opens one per press`, `electron/src/main/browser-pane.test.ts::keeps at most two popup windows open per tab`, `electron/src/main/browser-pane.test.ts::titles a popup window with its site first, whatever the page calls itself`

### A Claude thread at its usage limit offers Switch to on the limit line

- **Completed:** 2026-11-15T17:06:21-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `40267c5` (`fix/switch-limit-line-20261115`)
- **Notes:** When a Claude thread hits its usage limit, the line above the composer offers Switch to another provider beside Resume at reset
- **Commits:** A Claude thread at its usage limit offers Switch to on the limit line again
- **Pins:** `electron/src/renderer/thread-resume.test.ts::offers Switch to on a Claude limit that its turn's result already left out of the event log`

### Accounts: a removed account's threads stay listed and deletable; Sign in uses the thread's account

- **Completed:** 2026-11-15T17:01:17-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `e84d3c1` (`fix/accounts-review-20261115`)
- **Notes:** Remove an added account: its threads stay in Threads marked account removed. A signed-out thread's Sign in opens that thread's own account
- **Commits:** A thread's Sign in signs in the account the thread runs on; A removed account's threads stay listed until the owner deletes them
- **Pins:** `apps/interface-host/src/provider_accounts_removed_tests.rs::a_removed_accounts_sessions_stay_listed_after_the_next_listing`, `apps/interface-host/src/provider_accounts_removed_tests.rs::a_removed_accounts_sessions_stay_listed_and_bound_after_a_restart`, `apps/interface-host/src/provider_accounts_removed_tests.rs::a_removed_accounts_session_can_be_deleted_and_does_not_come_back`, `electron/src/renderer/thread-sign-in-account.test.ts::signs in the account the thread runs on, not the provider's default account`

### Team tools: starts sent together keep the 4-helper limit; finish notices arrive one at a time

- **Completed:** 2026-11-15T16:54:45-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `b2f1a1f` (`fix/team-tools-review-20261115`)
- **Notes:** Six helpers started at once stop at four running; helpers that finish together each show their own finished message in the caller
- **Commits:** A new thread's first team tool call waits for its bind when the project is reached through a link; Team tool starts that arrive together never pass the running limit together; Notices of started threads that finish together reach their caller one turn at a time
- **Pins:** `apps/interface-host/src/threads_team_tests.rs::starts_at_once_never_pass_the_running_limit_together`, `apps/interface-host/src/threads_team_tests.rs::notices_of_threads_that_finish_together_go_one_turn_at_a_time`, `apps/interface-host/src/threads_team_tests.rs::a_team_call_before_its_bind_waits_when_the_project_path_is_a_link`

### Outside agents: a retried start no longer makes a second thread; long calls and project names work

- **Completed:** 2026-11-15T16:50:20-06:00
- **Kind:** fix
- **Release:** v0.3.179
- **Commit:** `4d195a5` (`fix/outside-agents-review-20261115`)
- **Notes:** No change on screen: an outside agent (iface mcp) retrying a start gets the same thread and can list threads by a project's name
- **Commits:** Outside agents: list_threads takes a project's name as well as its root; iface mcp: a call longer than Interface reads is refused unsent; Outside agents: a start retried after a lost connection answers the first thread; iface mcp: a connection Interface closes unanswered is not "Interface is not running"
- **Pins:** `apps/interface-host/src/threads_outside_tests.rs::an_outside_start_retried_from_a_new_session_answers_the_first_thread`, `apps/interface-host/src/threads_outside_tests.rs::an_outside_list_filters_by_a_project_name_as_well_as_its_root`, `apps/iface/src/mcp_tests.rs::a_call_longer_than_interface_reads_is_refused_unsent_and_the_session_goes_on`, `apps/iface/src/mcp_tests.rs::a_connection_interface_closes_unanswered_is_not_called_interface_not_running`

### Theme files that can move carry a play mark in Settings

- **Completed:** 2026-11-15T14:45:38-06:00
- **Kind:** look
- **Release:** v0.3.179
- **Commit:** `572f062` (`feat/moving-theme-mark-20261115`)
- **Notes:** Settings › Appearance › Yours: Aurora, Paper Tide, Neon Grid, TN Code, Orbit and Glyph show a small play mark
- **Pins:** `electron/src/renderer/components/SettingsPage.test.ts::marks the theme files whose backdrop can move`, `crates/interface-settings/src/custom_themes.rs::says_whether_a_theme_paints_a_backdrop_and_whether_it_can_move`

### Terminal text is the terminal font again, not tiny sans-serif

- **Completed:** 2026-11-15T14:33:14-06:00
- **Kind:** fix
- **Release:** v0.3.178
- **Commit:** `685f81c` (`fix/terminal-font-20261115`)
- **Notes:** Terminal panes show normal-size monospace text again, in Dev and after updating the regular app
- **Pins:** `electron/src/renderer/canvas-font.test.ts::is drawn in the font xterm asked for, not the canvas's 10px default`, `electron/src/renderer/canvas-font.test.ts::is in place before any terminal is made`

### Record the TN Code themes landing

- **Completed:** 2026-11-15T13:57:13-06:00
- **Kind:** record
- **Release:** v0.3.178
- **Commit:** `f070f57` (`feat/tn-code-themes-20261115`)
- **Notes:** Record the TN Code themes landing

### TN Code's themes come to Interface, with shader backdrops for Orbit and Glyph

- **Completed:** 2026-11-15T13:56:42-06:00
- **Kind:** feature
- **Release:** v0.3.178
- **Commit:** `b6274e1` (`feat/tn-code-themes-20261115`)
- **Notes:** After copying the themes: TN Code, Orbit and Glyph show their moving patterns behind the panes in Dev
- **Pins:** `electron/src/renderer/theme-look.test.ts::draw Orbit's horizon behind TN Code and Orbit, and Glyph's dots behind Glyph`, `electron/src/renderer/theme-look.test.ts::is drawn when its own rate says a frame is due, under the backdrop's stepper`

### Browser pages rest under flipped cards, and dropped downloads say they failed

- **Completed:** 2026-11-15T13:52:33-06:00
- **Kind:** fix
- **Release:** v0.3.178
- **Commit:** `95820c9` (`fix/browser-followups`)
- **Notes:** Flip panes to cards with a video page open: the page stops running underneath; a download that loses its connection says it could not be downloaded
- **Commits:** A Browser page under a flipped card rests like a background tab instead of running at full speed; A Browser download whose connection drops says it failed instead of "Downloading…" forever; Say why the Browser tells sites camera, microphone and notifications are blocked before they ask; Pin that a Browser tab's sign-in popup closes when the tab closes; Make the Browser lane's fullscreen check say what is true in the real App
- **Pins:** `electron/src/main/browser-pane.test.ts::fails a download whose connection drops, frees its name and cancels it`

### Record the looks battery landing

- **Completed:** 2026-11-15T12:57:54-06:00
- **Kind:** record
- **Release:** v0.3.178
- **Commit:** `a1febe5` (`perf/looks-battery-20261115`)
- **Notes:** Record the looks battery landing

### Moving backdrops cost about half as much, and rest on battery when idle

- **Completed:** 2026-11-15T12:57:20-06:00
- **Kind:** speed
- **Release:** v0.3.178
- **Commit:** `08e5b62` (`perf/looks-battery-20261115`)
- **Notes:** Pick Aurora in Dev: it still moves, and Settings › Appearance shows Still on battery under Moving backdrops
- **Pins:** `electron/src/renderer/theme-look.test.ts::is drawn half as often on the battery, and rests there after a minute without input`, `electron/src/renderer/theme-look.test.ts::writes a blur into the page only when the theme file asks for one`

### The in-app Browser works like a Chrome tab

- **Completed:** 2026-11-15T10:55:40-06:00
- **Kind:** feature
- **Release:** v0.3.178
- **Commit:** `03af8a8` (`feature/browser-chrome-like`)
- **Notes:** Right-click menus, new tabs from links, sign-in popups, downloads, ⌘F, zoom, camera/mic prompts, fullscreen video and PDFs work in a Browser tab
- **Commits:** A Browser tab behaves like a Chrome tab: right-click menu, links in new tabs, sign-in popups, downloads, find, zoom and page keys, permission prompts, fullscreen and PDFs; Spell the download-name regex's double quote as an escape so the contract scan reads the file; The Browser's permission sheet defaults to Block, and pages may write to the clipboard as in Chrome
- **Pins:** `electron/src/main/browser-pane.test.ts::asks for the camera, microphone, notifications and the clipboard, allows fullscreen and refuses the rest`, `electron/src/main/browser-pane.test.ts::opens new tabs and windows the way Chrome does`, `electron/src/main/browser-pane.test.ts::makes Block the sheet's default, so only a deliberate Allow grants`, `electron/src/main/browser-pane.test.ts::pins a popup window exactly as a page, with no preload and no Node`

### Record the looks landing

- **Completed:** 2026-11-15T10:33:40-06:00
- **Kind:** record
- **Release:** v0.3.178
- **Commit:** `803b338` (`feat/theme-looks-20261115`)
- **Notes:** Record the looks landing

### Looks: theme files can paint a moving backdrop behind glass panes and threads

- **Completed:** 2026-11-15T10:33:05-06:00
- **Kind:** feature
- **Release:** v0.3.178
- **Commit:** `b3a0466` (`feat/theme-looks-20261115`)
- **Notes:** Pick Aurora under Yours in Settings › Appearance: one moving picture runs behind the sidebar, panes and threads
- **Commits:** Looks: record the settled-look approval the landing waits on; Looks: record the owner's approval for glass pane heads and the moving backdrop
- **Pins:** `electron/src/renderer/theme-look.test.ts::turns panes, threads and the sidebar to glass only while a backdrop is painted`, `electron/src/renderer/theme-look.test.ts::runs only while Interface is in front, motion is allowed and Settings leaves it on`, `electron/src/renderer/theme-look.test.ts::keeps what draws and drops scripts, foreign content, links and handlers`

### Browser opens as a tab in the pane you open it from

- **Completed:** 2026-11-15T10:12:28-06:00
- **Kind:** look
- **Release:** v0.3.178
- **Commit:** `0fd4320` (`look/browser-tab-in-pane`)
- **Notes:** Choosing Browser from a pane's + adds a Browser tab to that pane with the address bar ready; no pane splits or moves
- **Commits:** Choosing Browser from a pane's + menu or the palette adds a Browser tab to that pane instead of splitting it
- **Pins:** `electron/src/renderer/terminal-workspace.test.ts::opens from a pane as a tab in that pane, shown and focused, with no pane added or moved`

### Record the 0.3.177 release of the T3-style features

- **Completed:** 2026-11-15T09:03:16-06:00
- **Kind:** record
- **Release:** v0.3.178
- **Commit:** `13577d3` (`record/t3-release-20261115`)
- **Notes:** Record the 0.3.177 release of the T3-style features

### A new thread's first team tool call waits for the thread instead of saying it has no project

- **Completed:** 2026-11-15T04:26:38-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `0e2711d` (`fix/team-tools-first-prompt-20261115`)
- **Notes:** An agent asked in a new thread's first prompt to start or read threads does it, instead of saying the thread has no project folder
- **Commits:** Tests reproduce a new thread's first team tool call answered as if the thread had no project (fail until the fix); A new thread's first team tool call waits for the thread to be filed instead of answering that it has no project; Docs: a new thread's team tool call waits for its bind
- **Pins:** `apps/interface-host/src/threads_team_tests.rs::a_team_call_before_its_threads_bind_waits_for_the_bind`

### Record the twelve T3-style features as landed, with follow-ups and owner decisions

- **Completed:** 2026-11-15T04:19:18-06:00
- **Kind:** record
- **Release:** v0.3.177
- **Commit:** `d8bf4e1` (`record/t3-systems-20261115`)
- **Notes:** Record the twelve T3-style features as landed, with follow-ups and owner decisions

### Quick wins: Write with AI for commit and PR text, a private secret card, and side questions

- **Completed:** 2026-11-15T04:17:33-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `5d45ca8` (`feat/quick-wins-20261115`)
- **Notes:** Write with AI fills commit and PR text; an agent's secret request takes a value into a private file only; /btw asks a side question shown on the right
- **Commits:** Write with AI fills the commit message and the pull request's text from the thread's changes (protocol 67); An agent can ask for a secret: a card above the composer takes it into a private file, never the conversation; /btw asks a thread a side question in a hidden read-only fork; the answer shows on the right with Bring back and Close; Docs: quick wins contract (T3 section 12), protocol v67 facts, map and area notes; Record the project Away suite's protocol 67 version assertion as superseding its pin
- **Pins:** `apps/interface-host/src/threads_ship_tests.rs::write_with_ai_reads_only_what_ships_and_changes_nothing`, `apps/interface-host/src/threads_team_tests.rs::a_given_secret_exists_only_in_its_owner_only_file`, `apps/interface-host/src/threads_team_tests.rs::a_side_question_forks_the_thread_hidden_and_read_only_and_goes_when_closed`
- **Supersedes:** c79f64e2 — host protocol 67 (quick wins); the project Away suite test only asserts the version number; 497156a — host protocol 67 (quick wins); the outside agents suite test only asserts the version number

### Resume at reset: a thread that hits its usage limit carries on by itself after the reset

- **Completed:** 2026-11-15T03:48:33-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `62b88b6` (`feat/resume-at-reset-20261115`)
- **Notes:** When a thread hits its usage limit, the line offers Resume at reset and Snooze until reset; Resume carries the thread on by itself a minute after the reset
- **Commits:** Threads remember when a usage limit resets, and Resume at reset schedules the thread to carry on then (Rust host, protocol 64); The limit line offers Resume at reset and Snooze until reset, and a resume reads Resumes at <time> · Cancel; Docs: resume at reset contract as built, code map, area docs, agent map and the hands-free limit verb
- **Pins:** `apps/interface-host/src/threads_schedules_tests.rs::resume_at_reset_carries_the_thread_on_a_minute_after_the_reset_once`, `electron/src/renderer/thread-resume.test.ts::draws Resume at reset and Snooze until reset beside Switch to, and alone when there is nowhere to go`
- **Supersedes:** c79f64e2 — host protocol 66 (resume at reset); the project Away suite test only asserts the version number; 497156ad — host protocol 66 (resume at reset); the outside agents suite test asserts the host version number; c79f64e2 — host protocol 64 (the project Away suite test only asserts the version number)

### A thread whose turn finishes quickly no longer shows as stopped instead of done

- **Completed:** 2026-11-15T03:44:24-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `e3ab161` (`fix/finished-run-not-stopped-20261115`)
- **Notes:** A turn that finishes in a moment reads done and shows as unread; team waits, notices and scheduled prompts no longer see a finished run as stopped
- **Commits:** A test reproduces a quickly finished turn filed as stopped when a reconcile lands before its end is recorded (fails until the fix); A quickly finished turn is no longer filed as stopped: reconcile leaves a run alone while its end is on its way to the store; Docs: reconcile leaves a run whose end is on its way to the store; The outside agents end-to-end test requires a finished thread to read done again
- **Pins:** `apps/interface-host/src/threads_tests.rs::a_finished_run_whose_end_is_still_queued_is_not_filed_as_stopped_by_a_reconcile`
- **Supersedes:** 497156a — the finished-run race is fixed, so the end-to-end test now requires done

### Outside agents: Claude Code or Codex in a terminal can read and start threads (iface mcp)

- **Completed:** 2026-11-15T03:31:07-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `56a16ba` (`feat/outside-agents-20261115`)
- **Notes:** Settings, Threads, Outside agents lets a Claude Code or Codex in any terminal read your threads and, if allowed, start ones that ask before acting
- **Commits:** Outside agents: a Claude Code or Codex outside Interface can read threads and, when allowed, start and message them through iface mcp; Settings → Threads has Outside agents (Off, Read only, Can start and message threads) with each tool's setup line; a thread an outside agent started says so in its pane head (protocol 65); A real iface mcp against a real host proves an outside agent lists, starts, waits for and reads a thread; docs record the outside agents contract (protocol 65); The gate runs the Rust suite when the hands-free fake providers change, since the outside agents test runs them
- **Pins:** `apps/interface-host/tests/suite/outside_agents.rs::iface_mcp_lists_starts_waits_for_and_reads_a_thread_on_a_real_host`, `apps/interface-host/src/threads_outside_tests.rs::an_outside_start_runs_in_a_known_project_at_the_asking_rung_without_team_tools`
- **Supersedes:** c79f64e2 — host protocol 65 (outside agents); the project Away suite test only asserts the version number

### Quote a reply, send terminal text or another thread into a draft, and stash drafts

- **Completed:** 2026-11-15T03:06:15-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `efd7234` (`feat/composer-references-20261115`)
- **Notes:** Select reply text and click Quote; Cmd-S stashes a draft and Stashed (n) brings it back; a Thread chip sends that thread's title and last exchange
- **Commits:** Threads can stash drafts in Rust and refer to other threads of their project (protocol 63 host side); Threads composers quote replies, stash drafts and refer to other threads; terminals send a selection to a thread; Renderer lanes quote a reply, stash and restore a draft, add a thread chip and send a terminal selection; Stash rows carry the menu's glyph; Docs: composer references and stash contract, protocol v63 facts, map and area notes; Two more protocol 63 assertion sites: the unmigrated-operation count and the project Away suite's version
- **Pins:** `apps/interface-host/src/threads_references_tests.rs::a_thread_reference_becomes_the_block_rust_writes_from_the_journal`, `electron/src/renderer/composer-quote.test.ts::goes after the typed draft, one blank line apart, with the caret after it`
- **Supersedes:** c79f64e2 — host protocol 63 (composer references and stash); the project Away suite test only asserts the version number

### Scheduled prompts in a thread, and Watch on a pull request wakes the agent

- **Completed:** 2026-11-15T02:40:06-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `221227b` (`feat/thread-schedules-20261115`)
- **Notes:** Schedule a prompt in a thread's menu sends it later as a folded Scheduled prompt the agent answers; a Next line shows above the composer
- **Commits:** Threads can send a prompt on a schedule and watch a pull request (Rust host, protocol 62); Threads panes can schedule a prompt, show the next one above the composer, watch a pull request, and list schedules in Settings; The hands-free fake Claude can schedule a prompt to its own thread (team schedule); Docs: scheduled prompts and pull-request watch contract as built, code map, area docs and protocol 62 facts; Scheduled prompts' markup is tested, and a send left marked as going out before a restart reads as unknown; Docs: scheduled prompts' markup test named in the acceptance list
- **Pins:** `apps/interface-host/src/threads_schedules_tests.rs::a_due_schedule_fires_once_into_an_idle_thread_as_a_host_prompt`, `electron/src/renderer/thread-schedules.test.ts::refuses an interval under five minutes, a past time and an empty prompt`
- **Supersedes:** c79f64e2 — host protocol 62 (scheduled prompts); the project Away suite test only asserts the version number; c79f64e2 — host protocol 62 (the project Away suite test only asserts the version number)

### Visual replies: an agent can show a chart or mockup as a rendered card above its reply

- **Completed:** 2026-11-15T02:31:57-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `fde043e` (`feat/visual-replies-20261115`)
- **Notes:** Ask an agent in a thread for a chart: a card shows the drawn page above its reply, with Source and Open larger; it is still there after reopening
- **Commits:** Agents in a thread can show the owner a rendered page: Rust keeps, journals and replays each visual (protocol 61); A visual reply shows as a card with its page in a sandboxed frame, Source and Open larger; Output preview can render .html and .svg files; The fake Claude shows a chart for `visual <title>`; a DOM lane proves the visual frame draws and cannot reach the app, storage or the network; A reopened thread shows each visual right after the show_visual call that made it; the lane also renders an .html file in Output preview; Docs: visual replies contract as built, protocol 61 facts, code map, area docs and the hands-free visual verb; The iface terminal names a visual reply in one line instead of drawing it; test code passes the workspace clippy
- **Pins:** `apps/interface-host/src/threads_team_tests.rs::show_visual_keeps_journals_and_shows_the_page_once_and_refuses_with_codes`, `electron/src/main/visual-frames.test.ts::lets only an empty subframe load a visual, and nothing move one afterwards`, `electron/src/shared/visual-frame.test.ts::gives the page no network, storage or forms: only inline code and data: media`
- **Supersedes:** c79f64e2 — host protocol 61 (visual replies); the project Away suite test only asserts the version number

### Several accounts per provider: add accounts in Settings and pick one for a new thread

- **Completed:** 2026-11-15T02:08:53-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `105b4db` (`feat/provider-accounts-20261115`)
- **Notes:** Settings, Models and providers lists each provider's accounts; a new thread can run on another Claude, Codex or Grok account, and its head names it
- **Commits:** Provider accounts: wire shapes, fixture and the two account stores; Provider accounts: each CLI runs with its account's directory, and one adapter per account; Provider accounts: Settings list, account choice on new threads, per-account usage, host routing and terminal panes; Provider accounts: Claude pane capture finds added accounts, docs and map, clippy; Provider accounts: area docs; The New thread card's account control says it is an account; Record the pinned tests the accounts landing changed
- **Pins:** `apps/interface-host/src/provider_accounts_tests.rs::a_session_launched_on_an_account_is_bound_and_resumes_and_reads_history_there`, `electron/src/renderer/composer-thread.test.ts::puts an account control beside the model only when the provider has several accounts`
- **Supersedes:** 6a98cff — CreateTerminalRequest gains the optional account field (protocol 60); the sign-in test's request literal names it as None, behaviour unchanged; c79f64e2 — host protocol 60 (provider accounts); the project Away suite test only asserts the version number

### Switch to <provider> carries a thread on another provider and back, in the same thread

- **Completed:** 2026-11-15T01:43:33-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `9663273` (`feat/switch-provider-20261115`)
- **Notes:** Switch to Grok in a thread's menu keeps the same row and conversation; Grok gets a handoff, and switching back tells Claude only what it missed
- **Commits:** Threads store v5: a thread whose provider is switched keeps its row, pin and folder on its new session; A switched thread's journals read as one conversation, and its handoff and switch-back prompts are built from them; Protocol 58: Switch provider keeps the same thread, hands it over from the journal and switches back with only what was missed; Threads offer Switch to <provider> instead of Continue with: the pane, draft and attachments follow the thread to its new session; Hands-free runs get a scripted Grok over ACP, so a thread can switch providers without a real account; A switched thread's pane shows the earlier conversation at once, its old session leaves the inbox, and an ACP handoff shows as the launch prompt; Docs: Switch provider contract as built, code map, area docs, protocol 58 facts and the hands-free Grok; the fake Grok paces its updates; Switch provider code passes the workspace clippy; Docs: how a switched pane is drawn at the switch, provider words, and an ACP status ordering seen with the fake
- **Pins:** `apps/interface-host/src/threads_switch_tests.rs::switching_to_a_new_provider_hands_the_same_thread_over_from_its_journal`, `electron/src/renderer/thread-switch.test.ts::moves the pane, draft and attachments to the answered head, then shows it`
- **Supersedes:** c79f64e2 — host protocol 59 (the project Away suite test only asserts the version number); 4ef57672 — Switch to <provider> keeps the same thread instead of starting a new one; c79f64e2 — host protocol 58 (switch provider); the retained Away integration test now expects the ready frame at version 58, loop behavior unchanged; df9def16 — the Threads store is version 5 now, so an old version 1 store carries over to threads.v5.json as version 5; 15b7ca64 — the Threads store is version 5 now, so an old version 2 store carries over to threads.v5.json as version 5

### To-do list tidied: finished items move to the done file; code review fixes recorded

- **Completed:** 2026-11-15T01:35:21-06:00
- **Kind:** record
- **Release:** v0.3.177
- **Commit:** `055cc8a` (`record/review-fixes-todo`)
- **Notes:** The to-do list keeps open work; finished items move to the done file

### Electron security update (43.8.10) and dev-tool advisory fixes

- **Completed:** 2026-11-15T01:32:54-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `5975572` (`internal/dependency-advisories`)
- **Notes:** The app runs on Electron 43.8.10, and the dev tools carry their security fixes

### Collaboration contract code split into smaller files

- **Completed:** 2026-11-15T01:26:30-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `630d1ae` (`internal/collaboration-state-split`)
- **Notes:** The collaboration contract file is split into five modules along its sections

### A finished Codex turn leaves its thread finished and unread, not stuck in an open run

- **Completed:** 2026-11-15T01:20:06-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `9e3f336` (`fix/codex-idle-thinking-no-run-20261115`)
- **Notes:** After a Codex reply finishes, its thread reads finished and unread right away instead of still running until the session closes
- **Commits:** Provider replay runs with the thread journal on and checks a simple thread's journal; Tests: a finished Codex turn must leave its thread finished and unread, with no run left open; A finished Codex turn leaves its thread finished and unread instead of opening a run that never ends; Row 4 no longer calls the replay bugs open
- **Pins:** `apps/interface-host/src/threads_tests.rs::thinking_that_stops_opens_no_run_but_still_counts_inside_one`, `apps/interface-host/tests/suite/provider_replay.rs::codex_simple_replays_through_the_runtime_and_threads`

### Stopping a Claude turn ends it as interrupted, not failed, and opens no extra thread run

- **Completed:** 2026-11-15T01:13:13-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `31dbf43` (`fix/claude-stop-not-failure-20261115`)
- **Notes:** Stop a Claude reply mid-answer: the thread shows the turn as stopped, the session goes idle, and no error appears
- **Commits:** Tests: stopping a Claude turn must end it idle and interrupted, with no error or extra run; Stopping a Claude turn ends it idle and interrupted, without an error or an extra thread run
- **Pins:** `apps/interface-host/tests/suite/provider_replay.rs::claude_interrupt_replays_through_the_runtime_and_threads`

### Away's worker runner sheds leftover pane-delegation code

- **Completed:** 2026-11-15T01:09:17-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `d35d510` (`internal/delegation-leftovers`)
- **Notes:** The Away worker runner drops the pane-delegation parts nothing used

### Old finished team workers stay out of Threads and the Chat list

- **Completed:** 2026-11-15T00:59:27-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `8056d21` (`fix/threads-hide-old-workers`)
- **Notes:** After many agent teams, old finished workers no longer reappear as new threads at the top of Threads or as rows in the Chat list
- **Commits:** Threads keeps old finished orchestration workers hidden past 64 agents; Chat's session list hides old finished orchestration workers too (protocol 57)
- **Pins:** `apps/interface-host/src/threads_tests.rs::an_old_finished_worker_past_the_snapshot_bound_never_becomes_a_thread`, `apps/interface-host/src/orchestration_transport.rs::owner_and_client_name_every_worker_past_the_snapshot_bound`, `crates/interface-orchestration/tests/orchestration.rs::worker_session_keys_name_every_worker_the_control_snapshot_drops`, `electron/src/renderer/use-threads.test.ts::keeps the hidden sessions' identity while a newer view names the same ones`

### The gate catches update-log format changes, stale JS installs and hung Rust tests

- **Completed:** 2026-11-15T00:47:41-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `56e0755` (`internal/gate-receipt-and-timeout`)
- **Notes:** A change to the update log's format now runs the Electron parser test too; The Electron gate receipt now follows the installed node_modules, not only the lockfile; A hung Rust test executable now fails the gate after ten minutes instead of holding it forever

### Agents in a thread can start, wait for, read and message other threads

- **Completed:** 2026-11-15T00:36:49-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `2149d26` (`feat/team-tools-20261114`)
- **Notes:** Ask an agent to start a helper thread: it shows in the inbox as Started by that thread, and the first agent gets its answer back
- **Commits:** Team tools rules and ledger, and the Threads store keeps started threads (store v4); Claude, Codex and every ACP provider give an ordinary thread the team tools server when the host offers it; Host serves team tools: a socket per thread, start, wait, read, list, message and stop other threads, and one notice when a started thread finishes; Protocol 57: started threads carry a child lineage and team tools can be turned off; host tests for every team tool; Team tools tests pass clippy; A started thread's head says who started it, its caller shows a Started threads line, and Settings → Threads can turn team tools off; Hands-free fake Claude can start and wait for a thread through team tools; Docs: team tools contract as built, code map, area docs, protocol facts and hands-free verbs; Settings test counts the team tools switch; Team tools code passes the workspace clippy; Team tools: a Codex caller's never-ask mode counts as edits, and the receipts bound test fills the store in one write; Old Threads stores carry over to version 4; Format the team tools receipts test; Team tools receipts test: drop an unneeded mut
- **Pins:** `apps/interface-host/src/threads_team_tests.rs::start_thread_files_a_child_thread_sends_its_task_and_never_widens_its_mode`, `crates/interface-settings/src/thread_team_tests.rs::a_started_thread_never_runs_wider_than_its_caller_for_every_engine_pair`
- **Supersedes:** 15b7ca6 — the Threads store is version 4 now, so an old version 2 store carries over to threads.v4.json rather than threads.v3.json

### Phone access stays reachable when another device ties up the Mac's phone listener

- **Completed:** 2026-11-15T00:27:34-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `2e40c19` (`fix/phone-listener-limits`)
- **Notes:** With phone access on, the iPhone still connects nearby and through the relay; nothing looks different
- **Commits:** Phone access stays reachable while another device holds slow or idle connections to the Mac
- **Pins:** `crates/interface-remote/src/http/direct_tests.rs::uploads_that_never_send_their_body_hold_no_request_slot`, `crates/interface-remote/src/http/direct_tests.rs::the_listener_caps_connections_and_closes_silent_ones`, `crates/interface-remote/src/http/direct_tests.rs::the_listener_drops_a_peer_outside_the_nearby_networks_before_reading`, `crates/interface-remote/src/http/direct_tests.rs::every_nearby_response_closes_its_connection`

### Sends and approval answers no longer wait behind an agent's disk writes

- **Completed:** 2026-11-15T00:13:59-06:00
- **Kind:** speed
- **Release:** v0.3.177
- **Commit:** `c54f34e` (`speed/agent-guard-off-async`)
- **Notes:** Sending a message or answering an approval in any chat no longer freezes briefly while an agent is saving its task progress
- **Commits:** Sends and approval answers no longer wait on the shared async threads while an agent's task file is being written
- **Pins:** `apps/interface-host/src/agents_tests.rs::a_send_waits_for_the_agent_store_off_the_async_thread`

### The second app copy stops re-checking Muse and Antigravity plugins at every start

- **Completed:** 2026-11-15T00:08:04-06:00
- **Kind:** speed
- **Release:** v0.3.177
- **Commit:** `6bbcf71` (`speed/launch-plugin-marker`)
- **Notes:** Whichever of Interface or Interface Dev did not install the checkpoint plugins no longer runs three provider commands before its window connects
- **Commits:** The app copy that did not install the Muse and Antigravity checkpoint plugins stops re-checking them on every start
- **Pins:** `apps/interface-host/src/native_checkpoint_setup.rs::plugins_registered_by_the_other_channel_are_checked_once`

### Saved reviews of images and PDFs keep each snapshot once, in its own file

- **Completed:** 2026-11-15T00:04:25-06:00
- **Kind:** speed
- **Release:** v0.3.177
- **Commit:** `70d2985` (`speed/output-reviews-snapshots`)
- **Notes:** Typing a review note saves at once even with large images saved, and 32 image or PDF reviews fit instead of about two
- **Commits:** Saving a review note no longer rewrites the saved image or PDF
- **Pins:** `crates/interface-settings/src/output_reviews.rs::a_note_save_rewrites_only_the_small_metadata_file`, `crates/interface-settings/src/output_reviews.rs::a_version_1_store_is_carried_over_once_and_left_in_place`

### Recorded Claude and Codex sessions replay in tests; cargo xtask prove checks a fix

- **Completed:** 2026-11-15T00:02:38-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `353c788` (`feat/provider-replay-20261114`)
- **Notes:** Provider replay: a test-only recorder and replayer for provider CLI sessions; Provider replay: recorded Claude and Codex sessions now replay through the real runtime and threads; cargo xtask prove: a pinned test must fail on the base and pass here; replay documented; Provider replay: satisfy the gate's lint on one test; Mark provider replay built and awaiting landing; Replay tests build against the thread host that has a journal

### New chats keep working after 256 recovered sessions

- **Completed:** 2026-11-14T23:59:06-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `a3b3775` (`fix/prompt-recovery-eviction`)
- **Notes:** Starting a new chat or thread never fails with 'Prompt recovery is full' unless every saved prompt is still unresolved
- **Commits:** New chats keep sending once prompt recovery holds 256 sessions
- **Pins:** `crates/interface-settings/src/prompt_recovery.rs::a_new_session_past_the_cap_replaces_the_oldest_settled_receipt`, `crates/interface-settings/src/prompt_recovery.rs::unknown_outcomes_and_launch_guards_are_never_evicted`, `crates/interface-settings/src/agent_starts.rs::a_new_request_past_the_limit_replaces_the_oldest_settled_intent`

### iface follows provider updates, reports uncertain sends honestly, and spares reused process numbers

- **Completed:** 2026-11-14T23:56:25-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `6f27b3c` (`fix/iface-provider-path-retry-kill`)
- **Notes:** An iface left open across a Claude or Codex update keeps working; a send cut off mid-way exits 4 and says check first; iface kill spares a reused pid
- **Commits:** An open iface keeps launching Claude and Codex through their links after an update; iface agent send, steer and start no longer say "retry" when the TUI may already have the request; iface kill and iface ps no longer trust a process number macOS has reused
- **Pins:** `apps/iface/src/app_tests.rs::provider_resolution_keeps_the_link_not_the_version_it_points_at`, `apps/iface/src/agent_client.rs::a_send_whose_reply_is_lost_after_the_write_is_outcome_unknown_not_retryable`, `apps/iface/src/background.rs::a_reused_pid_is_listed_stale_and_never_signalled`

### Long Claude chats open without re-reading the whole transcript

- **Completed:** 2026-11-14T23:50:59-06:00
- **Kind:** speed
- **Release:** v0.3.177
- **Commit:** `f9eb97e` (`speed/claude-long-chat-index`)
- **Notes:** Reopening a long Claude chat, or loading older messages in it, is quick instead of pausing while the whole transcript is read again
- **Commits:** Long Claude chats open again from a small index instead of re-reading the whole transcript
- **Pins:** `crates/engine-claude/src/store/history_cache.rs::tail_index_reads_match_fresh_reads_over_the_cache_cap`

### Copy Diagnostics hides access_token, refresh_token and similar values

- **Completed:** 2026-11-14T23:43:02-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `1e40c85` (`fix/diagnostics-token-filter`)
- **Notes:** A copied diagnostics report shows refresh_token=[redacted] instead of the value
- **Commits:** Copy Diagnostics now hides access_token=, refresh_token= and other *_token, *_secret and *_key values; The Copy Diagnostics token test covers access_token, refresh_token and client_secret
- **Pins:** `crates/interface-review/src/diagnostics.rs::copy_diagnostics_names_the_build_and_providers_and_never_carries_tokens`
- **Supersedes:** f78f0ff6 — the test now also requires access_token=, refresh_token= and client_secret values to be hidden; its earlier assertions are unchanged

### App's state blocks move into eight hooks; App.tsx is about 850 lines shorter

- **Completed:** 2026-11-14T23:39:48-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `260709a` (`internal/app-hooks`)
- **Notes:** The Threads bridges and second-opinion block live in their own hook; The thread output preview's state lives in its own hook; The file picker's store lives in its own hook; The per-turn Undo's capture and restore live in their own hook; The agent store's reads, mutations and poll live in their own hook; The subagent detail column's state lives in its own hook; The return recap lives in its own hook; Project Away's panel state lives in its own hook

### The iface TUI controller and settings commands move out of tui.rs

- **Completed:** 2026-11-14T23:33:23-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `fac2f79` (`internal/tui-split`)
- **Notes:** Move TuiController's methods and the launch-settings commands out of tui.rs

### Agents ask before commands hidden behind a zsh short if/while or runners like xcrun and uv run

- **Completed:** 2026-11-14T23:28:12-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `29aef5c` (`fix/agent-policy-runner-gaps`)
- **Notes:** An agent running 'if [[ -n x ]] git push' or 'uv run git push' now asks you first instead of running unasked
- **Commits:** Agents now ask before a command hidden behind a zsh short form or a command runner
- **Pins:** `crates/interface-desktop-services/src/agent_policy.rs::policy_table`

### Threads keep a durable record of every conversation, and the Threads search finds message text

- **Completed:** 2026-11-14T23:21:32-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `5e7162c` (`feat/thread-journal-20261114`)
- **Notes:** Type three or more letters from an old message into the Threads search: the thread appears under In messages with the matching text
- **Commits:** Record the T3-style systems program and its first contracts; Thread journal store: per-thread segments, bounds, replay, search and import; Threads keep a journal: the host writes prompts, answers and turn ends as they happen; Threads search finds message text: matches show under In messages; In messages caption stays inside the list; a launch prompt keeps journal times in order; Record the thread journal in the protocol facts, code map, area docs and its spec; The journal tap checks which events it keeps without copying them; Count the thread journal channel in the IPC bridge and channel list tests; Mark the thread journal built and awaiting landing; Orchestration test helpers wait up to 30 s, so a busy machine no longer fails them
- **Pins:** `apps/interface-host/src/threads_tests.rs::the_journal_worker_drains_and_its_reads_find_the_answer`, `electron/src/renderer/thread-inbox.test.ts::lists threads whose messages match under In messages, with the excerpt in the project's place`

### Agent Canvas Stop buttons work, and an agent's first chat no longer crashes the window

- **Completed:** 2026-11-14T23:19:05-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `114b45d` (`fix/canvas-stop-roster-contract-parity`)
- **Notes:** Stop worker, Stop session and Stop all workers on the Agent Canvas now stop the agent; an agent's first message no longer brings up the crash screen
- **Commits:** Agent Canvas Stop buttons work again, a first agent thread no longer crashes the window, and a test keeps Rust and TypeScript contracts in step
- **Pins:** `crates/interface-contracts/src/ipc.rs::orchestration_agent_requests_accept_both_agent_id_spellings`, `apps/interface-host/src/orchestration_tests.rs::stop_agent_accepts_the_camel_case_agent_id_the_app_sends`, `electron/src/renderer/agent-roster.test.ts::reads an agent whose first thread has no past list, as Rust sends it`, `electron/scripts/contract-parity.test.mjs::every same-named Rust struct and TypeScript interface agree on wire fields`

### Quitting cleans up the temporary worktrees of chats still open

- **Completed:** 2026-11-14T23:12:27-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `6b9fe3d` (`fix/quit-releases-chat-worktrees`)
- **Notes:** Quit with a chat open in a clean throwaway worktree: the worktree is gone after quit and the chat points back at its project folder
- **Commits:** Quitting now releases the clean temporary worktrees of chats that were still open
- **Pins:** `apps/interface-host/src/dispatcher_tests.rs::quit_after_the_desktop_receiver_is_gone_still_schedules_worktree_release`, `apps/interface-host/src/dispatcher_tests.rs::quit_after_an_unforwarded_provider_event_still_projects_close_and_release`, `crates/interface-runtime/tests/runtime.rs::a_registry_change_after_shutdown_reaches_the_cache_when_flushed`

### The update history loads only when you open Settings → Updates

- **Completed:** 2026-11-14T23:06:02-06:00
- **Kind:** speed
- **Release:** v0.3.177
- **Commit:** `8cbc908` (`speed/lazy-update-log`)
- **Notes:** The app's main script is 20% smaller (2.65 MB to 2.12 MB); the update log appears when you open Settings → Updates
- **Commits:** Load the update history only when Settings → Updates opens; Record that the Settings update-log test now waits for the lazily loaded panel; The Settings update-log test waits for the lazily loaded panel
- **Pins:** `electron/src/renderer/update-log.test.ts::loads the log and its panel only when Settings -> Updates opens`
- **Supersedes:** 2a98080 — the update log now loads in its own chunk when Settings → Updates opens; the test waits for it and asserts the same content

### Browser pane: point at an element or mark a region and add it to a thread's draft

- **Completed:** 2026-11-14T22:57:41-06:00
- **Kind:** feature
- **Release:** v0.3.177
- **Commit:** `9165b13` (`feat/browser-point-and-fix-20261114`)
- **Notes:** In a Browser tab press Pick and click an element: a card shows its picture, and Add to draft puts a note and the picture in the chosen thread's draft
- **Commits:** Browser pane can pick an element or mark a region and add a note with its picture to a thread's draft; Browser point-and-fix has a real-page DOM lane, keeps the page still while picking, and draws its width preset as a pill; Point-and-fix browser preview is documented: contract, area notes, code map and agent map
- **Pins:** `electron/src/renderer/browser-point.test.ts::appends the note and attaches the picture, never sending`

### Forking a chat no longer says it is safe to retry when the copy may exist

- **Completed:** 2026-11-14T22:42:00-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `abdff00` (`fix/fork-outcome-unknown`)
- **Notes:** If a fork hits a local error after the copy is made, Interface says the outcome is unknown and does not offer a blind retry
- **Commits:** Forking a chat no longer says "safe to retry" when the copy may already exist
- **Pins:** `crates/interface-runtime/tests/session_management.rs::a_registry_failure_after_the_provider_fork_is_outcome_unknown`

### Only the app shell talks to the backend, now checked by a test

- **Completed:** 2026-11-14T22:38:53-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `a57aae4` (`internal/window-iface-rule`)
- **Notes:** Only App.tsx and main.tsx reach the host from the renderer, and a test now says so

### The sidebar and hidden terminal screen stay still while a chat streams

- **Completed:** 2026-11-14T22:33:30-06:00
- **Kind:** speed
- **Release:** v0.3.177
- **Commit:** `cd5e5ca` (`speed/stable-thread-callbacks`)
- **Notes:** While a chat streams, the sidebar and the hidden Code screen stop redrawing every frame (196 redraws in 5 s became 1 and 0)
- **Commits:** Keep the sidebar and the hidden terminal screen still while a chat streams
- **Pins:** `electron/src/renderer/app-memo-props.test.ts::keeps the thread callbacks stable: no dependency array names the whole `threads` handle`, `electron/src/renderer/app-memo-props.test.ts::passes <OutputPreviewPanel> no inline callbacks or literals`

### Threads panes skip two Chat-only passes while streaming

- **Completed:** 2026-11-14T22:27:42-06:00
- **Kind:** speed
- **Release:** v0.3.177
- **Commit:** `5d772f0` (`speed/threads-skip-chat-summaries`)
- **Notes:** No visible change; a streaming Threads pane does about a tenth less JavaScript work per word
- **Commits:** A Threads pane no longer works out the Chat layout's turn chips and Fork ends on every streamed frame
- **Pins:** `electron/src/renderer/thread-layout-passes.test.ts::does not work out the Chat chip or the Chat Fork's turn ends`

### Thread line totals use one rule; two unused leftovers removed

- **Completed:** 2026-11-14T22:19:31-06:00
- **Kind:** internal
- **Release:** v0.3.177
- **Commit:** `c336d28` (`internal/renderer-leftovers`)
- **Notes:** The thread pane head and inbox row total a thread's lines by one rule, and two leftovers are gone

### Declined edits no longer count as changes in Chat

- **Completed:** 2026-11-14T22:14:35-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `b483cc5` (`fix/declined-edits-not-counted`)
- **Notes:** Deny an edit in a Chat pane: no '1 file changed' pill, turn chip, sidebar line count or recap claim for it
- **Commits:** Declined or failed edits no longer count as changes in the Chat layout, sidebar and recap
- **Pins:** `electron/src/renderer/diff-stats.test.ts::leaves a session whose only edit was declined with no stat and no paths`, `electron/src/renderer/conversation.test.ts::omits the pill when the only edit was declined`

### Fix pre-promote worktree, hook, startup, approval, queue-edit, search, and Codex reopen bugs

- **Completed:** 2026-11-10T11:30:04-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `45c4529` (`fix/pre-promote-regressions-20261110`)
- **Notes:** Deleted checkouts prune cleanly, Shell panes do not stall Muse, launch failure shows one alert, multi-approvals stay Waiting, queue edits and ⌘F behave
- **Commits:** fix: resolve seven pre-promote regressions across host, engines, and renderer
- **Pins:** `crates/interface-registry/src/worktree.rs::deleted_or_forced_ignored_checkout_removes_cleanly`, `apps/interface-host/src/native_tests.rs::cleanup_removes_deleted_checkout_without_verifying_missing_directory`, `apps/interface-host/src/worktree_release_tests.rs::deleted_checkout_is_released_even_when_verification_would_fail`, `apps/interface-host/src/provider_capture/native.rs::lineage_hook_does_not_wait_for_open_manual_shell_panes`, `electron/src/main/main-wiring.test.ts::destroys an already-created window and shuts down durably on top-level startup rejection`, `crates/engine-claude/src/adapter_tests.rs::concurrent_approvals_stay_waiting_until_last_request_is_settled_or_cancelled`, `crates/engine-codex/src/adapter_tests.rs::concurrent_approvals_stay_waiting_until_last_approval_is_confirmed`, `crates/engine-acp/src/adapter_tests.rs::concurrent_approvals_stay_waiting_until_last_permission_is_confirmed`, `electron/src/renderer/components/Conversation.queue.test.ts::releases session A's queue-edit lock when switching sessionKey A -> B`, `electron/src/renderer/outbound-lane.test.ts::clears queueEditing when a session is dropped so a recreated key is never wedged`, `electron/src/renderer/outbound-lane.test.ts::refuses editing a queued prompt into a slash command`, `electron/src/renderer/terminal-keys.test.ts::refuses ⌃⌘F, ⇧⌘F, ⌥⌘F and IME composition on macOS, and uses Ctrl+Shift+F elsewhere`, `crates/interface-settings/src/terminal_workspaces.rs::stale_codex_roster_cannot_undo_a_host_persisted_shell_demotion`
- **Supersedes:** a288070f — hostStarted waits until loaded, terminalBridge.ready, and requireUpdaterConfiguration succeed

### One alert when the backend fails while Interface is opening

- **Completed:** 2026-11-10T08:29:45-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `0c08aaa` (`fix/startup-failure-single-alert`)
- **Notes:** A backend that fails during launch shows only 'Interface could not open'; a later crash still offers Relaunch
- **Commits:** Show one alert when the backend fails before it has started
- **Pins:** `electron/src/main/main-wiring.test.ts::shows only the startup failure when the backend fails before it has started`

### A queued follow-up being edited is not sent until you save or cancel

- **Completed:** 2026-11-10T08:28:08-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `2f25234` (`fix/queued-edit-survives-turn-end`)
- **Notes:** Queue a follow-up, click Edit, let the agent finish: nothing is sent and your edit stays; Save sends the new wording
- **Commits:** Hold a queued follow-up's delivery while it is open for editing
- **Pins:** `electron/src/renderer/outbound-lane.test.ts::does not deliver a queued prompt open for editing when the turn ends`

### Muse hooks check once and exit unless a pane of this channel is still starting

- **Completed:** 2026-11-10T08:26:20-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `3ca7edb` (`fix/muse-hook-lineage-wait`)
- **Notes:** Muse Code no longer pauses about a second on every tool call, inside or outside Interface
- **Commits:** fix(muse): stop each Muse hook waiting half a second per channel
- **Pins:** `apps/interface-host/src/provider_capture/native.rs::lineage_hook_waits_only_while_a_process_is_not_yet_filed`

### A Kimi Code pane that starts a new chat reopens after a restart

- **Completed:** 2026-11-10T08:24:24-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `a2d4d0b` (`fix/kimi-pane-reopen`)
- **Notes:** In a Kimi pane use /new, then restart Interface: the pane comes back on the new chat
- **Commits:** fix(terminals): let a Kimi Code pane that starts a new chat reopen
- **Pins:** `apps/interface-host/src/bootstrap_tests.rs::every_captured_provider_session_change_reaches_the_saved_roster`

### A Claude session shows as working again after you approve its request

- **Completed:** 2026-11-10T08:22:39-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `40d8c72` (`fix/claude-working-after-approval`)
- **Notes:** Approve a Claude tool request: the Dock badge stops counting it as waiting and the Mac stays awake while it works
- **Commits:** fix(claude): show a Claude session as working again after an approval
- **Pins:** `crates/engine-claude/src/adapter_tests.rs::confirmed_approval_puts_the_session_back_to_working`

### Ending an isolated session no longer deletes git-ignored files or unregisters other worktrees

- **Completed:** 2026-11-10T08:20:41-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `3ca1f37` (`fix/registry-worktree-keeps-ignored`)
- **Notes:** End a session whose worktree has a .env or node_modules: the folder stays and a notice says it was kept
- **Commits:** fix(worktrees): keep a managed worktree that holds git-ignored files
- **Pins:** `crates/interface-registry/src/worktree.rs::removal_keeps_ignored_files_and_other_absent_worktrees`, `apps/interface-host/src/worktree_release_tests.rs::checkout_with_ignored_files_is_retained_once_with_a_notice_naming_them`

### Ctrl+F in a terminal pane reaches the program; only ⌘F opens search

- **Completed:** 2026-11-10T08:15:59-06:00
- **Kind:** fix
- **Release:** v0.3.177
- **Commit:** `67ed686` (`fix/terminal-ctrl-f`)
- **Notes:** In a terminal pane, Ctrl+F moves the cursor forward in the shell; ⌘F still opens the search box
- **Commits:** Let Ctrl+F in a terminal pane reach the program
- **Pins:** `electron/src/renderer/terminal-keys.test.ts::opens the terminal search box on ⌘F and leaves Ctrl+F to the program`

### Agent rules: subagents hand work back, and xtask may relaunch Dev

- **Completed:** 2026-11-09T14:09:15-06:00
- **Kind:** record
- **Release:** v0.3.177
- **Commit:** `0937757` (`rules/subagent-closeout`)
- **Notes:** Agent rules: subagents hand work back, and xtask may relaunch Dev

### The sidebar's working indicators keep moving while an agent works

- **Completed:** 2026-10-28T18:39:18-05:00
- **Kind:** look
- **Release:** v0.3.176
- **Commit:** `4f9958c` (`look/ticker-stepped-20261028`)
- **Notes:** Start an agent: the sidebar ticker and folder dot keep moving for the whole run, and still stop while another app is in front
- **Pins:** `electron/src/renderer/loop-rest.test.ts::rests only loops outside the sidebar and leaves the sidebar's to their rules`

### Working marks animate for a few seconds, then hold still, so busy agents stop redrawing nonstop

- **Completed:** 2026-10-27T16:24:37-05:00
- **Kind:** look
- **Release:** v0.3.175
- **Commit:** `f6739e4` (`look/loops-rest-20261027`)
- **Notes:** Start an agent: the sidebar ticker moves for about 3 s, then holds still; a new turn moves it again
- **Commits:** Working marks animate for a few seconds, then hold still
- **Pins:** `electron/src/renderer/loop-rest.test.ts::lets a loop run about three seconds, then holds the first frame of its cycle`

### Agents working in background panes use far less battery while another app is in front

- **Completed:** 2026-10-27T16:18:27-05:00
- **Kind:** speed
- **Release:** v0.3.175
- **Commit:** `dec9d2c` (`speed/battery-background-20261027`)
- **Notes:** With two or three agents working and another app in front, Interface's renderer and GPU drop from about 25% CPU to under 8%
- **Commits:** Background panes stop redrawing the window for every spinner frame

### Shift+Tab stays in its terminal pane

- **Completed:** 2026-10-27T12:35:27-05:00
- **Kind:** fix
- **Release:** v0.3.175
- **Commit:** `f740bff` (`fix/shift-tab-terminal-pane-20261027`)
- **Notes:** In a terminal pane, Shift+Tab cycles the model's permission mode and focus stays in that pane
- **Pins:** `electron/src/renderer/terminal-keys.test.ts::claims plain Shift+Tab so it reaches the program instead of moving pane focus`, `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps Shift+Tab in the pane as the program's reverse tab, never a focus move`

### Record that Kimi Code shipped in 0.3.174

- **Completed:** 2026-10-26T21:13:12-05:00
- **Kind:** record
- **Release:** v0.3.175
- **Commit:** `fc72ef1` (`record/kimi-shipped-20261026`)
- **Notes:** Record that Kimi Code shipped in 0.3.174

### The packaged smoke fakes kimi so release packaging sees the full launcher

- **Completed:** 2026-10-26T21:10:31-05:00
- **Kind:** internal
- **Release:** v0.3.174
- **Commit:** `956b1cd` (`fix/packaged-smoke-fake-kimi-20261026`)
- **Notes:** Fake kimi in the packaged smoke sandbox

### Record the Kimi follow-ups landing

- **Completed:** 2026-10-26T21:03:44-05:00
- **Kind:** record
- **Release:** v0.3.174
- **Commit:** `26fecc2` (`feat/kimi-code-followups-20261026`)
- **Notes:** Record the Kimi follow-ups landing in the TODO entry

### Kimi Code panes reopen their chat, show working and done, accept Pass off and get their phone mark

- **Completed:** 2026-10-26T21:02:55-05:00
- **Kind:** feature
- **Release:** v0.3.174
- **Commit:** `29e3368` (`feat/kimi-code-followups-20261026`)
- **Notes:** In a Kimi pane send a prompt and see working then done, restart Dev and it reopens the same chat, and Pass off into Kimi types the brief (needs kimi installed)
- **Commits:** Kimi Code panes resume, show activity, take Pass off and get a phone mark; Keep the pinned pass-off test intact; test the Kimi pass_off profile on its own
- **Pins:** `crates/interface-terminal/src/native_checkpoint.rs::kimi_binds_its_session_and_ends_a_turn_only_when_no_subagent_is_open`, `apps/interface-host/src/terminal_tests.rs::native_reopen_resumes_the_exact_kimi_session`, `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_kimi_pass_off_is_typed_once_after_its_editor_takes_a_paste`, `apps/interface-host/src/native_checkpoint_setup.rs::kimi_plugin_registers_every_observed_event_and_keeps_owner_plugins`

### Note the Dev-app check for Muse stop detection

- **Completed:** 2026-10-26T19:39:16-05:00
- **Kind:** record
- **Release:** v0.3.172
- **Commit:** `f8155f5` (`fix/muse-plugin-hooks-20261026`)
- **Notes:** Note the owner check for Muse stop detection after /clear

### Muse panes notice stops again after /clear

- **Completed:** 2026-10-26T19:37:05-05:00
- **Kind:** fix
- **Release:** v0.3.172
- **Commit:** `5025b85` (`fix/muse-plugin-hooks-20261026`)
- **Notes:** In a Muse pane, /clear then let it finish: the pane still switches to done and pings you
- **Commits:** Detect Muse stops after /clear via checkpoint plugin hooks; Format the Muse plugin hook changes
- **Pins:** `apps/interface-host/src/provider_capture/native.rs::lineage_scan_selects_the_generation_whose_process_matches`, `apps/interface-host/src/native_checkpoint_setup.rs::muse_plugin_bundle_routes_every_event_by_lineage`

### Kimi Code is available in the terminal launcher and + menu

- **Completed:** 2026-10-23T14:44:55-05:00
- **Kind:** feature
- **Release:** v0.3.172
- **Commit:** `eb4ff45` (`feat/kimi-code-terminal-20261023`)
- **Notes:** New pane → Kimi Code opens kimi in the chosen folder with the Kimi mark (needs kimi installed)
- **Commits:** Add Kimi Code as a terminal launch profile
- **Pins:** `crates/interface-contracts/src/terminal.rs::kimi_wire_round_trips_the_shared_protocol_v54_fixture`, `apps/interface-host/src/bootstrap_tests.rs::kimi_discovery_uses_the_installer_binary_but_skips_non_executables`, `electron/src/renderer/components/TerminalLauncher.test.ts::offers Kimi Code with its own mark beside the other agents`

### Panes in a hidden workspace keep the chosen terminal text size

- **Completed:** 2026-10-23T14:25:44-05:00
- **Kind:** fix
- **Release:** v0.3.171
- **Commit:** `f5ed798` (`fix/terminal-text-size-parked-20261023`)
- **Notes:** Every workspace shows the terminal text size chosen in Settings, including after a theme or light/dark change made while it was out of view
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::repaints a pane parked through an appearance change on its return, never from the parking container`

### To-do: the Threads look's open canvas details and its live check

- **Completed:** 2026-10-23T14:06:11-05:00
- **Kind:** record
- **Release:** v0.3.170
- **Commit:** `af60122` (`look/threads-claude-app-20261023`)
- **Notes:** To-do: the Threads look's landing and the canvas details still open

### Threads take the Claude app's Code mode look: new icons, the Pair working mark, roomier spacing

- **Completed:** 2026-10-23T14:04:42-05:00
- **Kind:** look
- **Release:** v0.3.170
- **Commit:** `2b501d2` (`look/threads-claude-app-20261023`)
- **Notes:** Open a thread: line icons, two beads cross while it works, no project glow, a roomier lane and inbox, Send in the theme accent
- **Commits:** Threads look: the icon set and the Pair thinking mark (foundation); Threads look: the working head line and the Changes flash take the accent; Threads look: the inbox in the image of Code mode (S1); Threads look S4: the Changes panel as the Review board draws it; Threads look: drop the header band's last plumbing; Threads look: the transcript and the live turn (S2); Threads look S3: composer, dock, pane head and New thread card in the Code mode look; Threads look: the docked request's tile says what it asks about; Threads look: no dangling slash in the head, one badge case; Threads look: the renderer guide and the work-chip note say what now draws; Settled looks: record the Threads Claude-app look
- **Pins:** `electron/src/renderer/components/ThreadGlyph.test.ts::draws every icon on the one 16px grid with a 1.5 round stroke, hidden from assistive tech`, `electron/src/renderer/components/ThreadGlyph.test.ts::moves at the app's stepped rate, rests in the background and holds under reduced motion`, `electron/src/renderer/thread-view.test.ts::draws the status line under the trail, with the Pair mark and never the Wave`, `electron/src/renderer/components/ThreadInbox.test.ts::keeps the project's hue to its square: none on the row, its selection, its unread dot or the header`, `electron/src/renderer/thread-hue.test.ts::reads no hue in the transcript or the composer: the theme's own accents draw them`
- **Supersedes:** 5eb108d — the Threads look draws the Changes panel's Undo turn as the header's icon button labelled "Undo turn" (the Review board), so the pinned busy check matches that button; Undo turn is still offered only on a local or owned-worktree thread's turn and disabled while busy; 48ff6fd9 — the Threads look drops "waiting " from the live clock beside "Waiting for you", so the pinned dock test reads the bare m:ss; 48ff6fd — the Threads look sets the inbox's live line after the project's hue square in the dim tier; "says on line two what a live thread is doing, asking or failed on, in the project line's place" now checks the square where the letter tile was

### Light terminal screens hold text to 7:1 so status lines and CLI colours stay readable

- **Completed:** 2026-10-23T10:23:55-05:00
- **Kind:** look
- **Release:** v0.3.170
- **Commit:** `32c8e1b` (`fix/light-screen-contrast-floor-20261023`)
- **Notes:** On Clay Light, Claude's status line (model, effort, tokens) and 'auto mode on' read in deep rust, forest and amber instead of faint orange and yellow
- **Commits:** fix(terminal): hold text to 7:1 on light screens
- **Pins:** `electron/src/renderer/terminal-palettes.test.ts::holds text to 7:1 on light screens and leaves dark screens alone`

### Markdown task lists draw checkboxes and indented lists nest under their item

- **Completed:** 2026-10-23T08:58:12-05:00
- **Kind:** fix
- **Release:** v0.3.170
- **Commit:** `d1aa18b` (`fix/markdown-task-nested-lists-20261023`)
- **Notes:** Open a Markdown file or reply with '- [x] done' and a bullet indented under a numbered item: boxes instead of [x], and the bullet sits under its item
- **Commits:** fix(markdown): draw task-list boxes and nest indented lists
- **Pins:** `electron/src/renderer/markdown.test.ts::draws task items as read-only boxes and nests indented lists under their item`

### File viewer looks like the rest of the app: pane head band, tab row, composer-style feedback

- **Completed:** 2026-10-23T08:53:04-05:00
- **Kind:** look
- **Release:** v0.3.170
- **Commit:** `6db451c` (`look/output-viewer-20261023`)
- **Notes:** Click a file link in a pane: the viewer's head lines up with the pane heads, open files are tabs in it, and feedback is a Threads-style box
- **Commits:** look(output-preview): file viewer chrome matches the panes and agent overview
- **Pins:** `electron/src/renderer/components/OutputPreviewPanel.test.ts::draws the panes' head band, ghost actions and the app's own pickers`

### Interface uses far less battery: animations rest when it is in the background

- **Completed:** 2026-10-22T12:29:00-05:00
- **Kind:** speed
- **Release:** v0.3.169
- **Commit:** `9fe35be` (`speed/battery-animations-20261022`)
- **Notes:** With an agent working and an update waiting, Interface's CPU drops to about 1% when another app is in front
- **Commits:** speed(renderer): rest looping animations off-focus and step them at 12 fps; record(looks): owner approved the looping motion budget
- **Pins:** `electron/src/renderer/window-activity.test.ts::steps every loop at about 12 frames a second and rests it while Interface is not in front`

### Fix Antigravity completion and permission indicators

- **Completed:** 2026-10-22T12:07:22-05:00
- **Kind:** fix
- **Release:** v0.3.169
- **Commit:** `202078f` (`fix/agy-completion-20261022`)
- **Notes:** Antigravity panes show Done when AGY finishes and keep permission waits visible despite capture hooks.
- **Commits:** fix(terminal): honor AGY native completion after capture hooks
- **Pins:** `crates/interface-terminal/src/service/workspace_sleep_fixture_tests.rs::agy_native_status_overrides_hooks_for_attention_and_workspace_sleep`, `electron/src/renderer/terminal-activity.test.ts::follows native AGY status despite interleaved hooks at every byte boundary`

### Open clickable AI response file links in the in-app file viewer

- **Completed:** 2026-10-22T09:58:40-05:00
- **Kind:** feature
- **Release:** v0.3.168
- **Commit:** `e0214c9` (`feat/clickable-response-files`)
- **Notes:** Click a file link or path in a Code terminal (including OMP) or a Threads/Agent Markdown response to open it in the side-by-side file viewer
- **Pins:** `apps/interface-host/src/terminal_tests.rs::preview_output_reads_agent_session_artifacts_without_granting_directory_listing`, `electron/src/renderer/terminal-links.test.ts::resolves OSC 8 file:// URLs with query or line fragments to clean local file paths`, `electron/src/renderer/markdown.test.ts::turns local file links into preview actions when an onOpenFile handler is provided`, `electron/src/renderer/prompt-images-ui.test.ts::turns local file links in assistant responses into clickable preview links`

### Add full update log in docs/UPDATES.md and Settings -> Updates

- **Completed:** 2026-10-21T16:54:33-05:00
- **Kind:** feature
- **Release:** v0.3.167
- **Commit:** `8be49c5` (`feat/update-log`)
- **Notes:** Settings -> Updates shows the searchable release and Dev landing update log, and docs/UPDATES.md stays synced on closeout and promote
- **Commits:** feat(updates): add full update log in docs/UPDATES.md and Settings -> Updates
- **Pins:** `xtask/src/update_log.rs::parse_and_render_update_log_links_releases_and_landings`, `xtask/src/closeout.rs::closeout_keeps_docs_updates_md_in_sync_across_sequential_and_concurrent_landings`, `electron/src/renderer/update-log.test.ts::parses releases, release items, landings, notes, pins, and completion timestamps`, `electron/src/renderer/components/SettingsPage.test.ts::shows a controlled update center without adding chrome when updater state is absent`

### Harden terminal pass-off across MCP tools, host dispatch, and workspace seating

- **Completed:** 2026-10-21T14:48:47-05:00
- **Kind:** fix
- **Release:** v0.3.167
- **Commit:** `53951df` (`fix/pass-off-review`)
- **Notes:** Passed-off panes launch for every profile, Codex delegates report back without approval prompts, and background pass-offs stay in their source workspace.
- **Pins:** `apps/interface-host/src/pane_mcp.rs::pass_off_builds_a_brief_create_at_the_callers_cwd_pointing_back_at_it`, `apps/interface-host/src/terminal_tests.rs::send_to_pane_rejects_an_exited_pane`, `apps/interface-host/src/bootstrap_tests.rs::codex_pane_capture_argv_keeps_its_exact_order`

### Name Antigravity panes from the conversation title

- **Completed:** 2026-10-21T08:52:32-05:00
- **Kind:** feature
- **Release:** v0.3.166
- **Commit:** `6523980` (`feat/antigravity-pane-titles`)
- **Notes:** An Antigravity pane adopts its generated conversation title or first prompt instead of staying named Antigravity
- **Pins:** `crates/interface-terminal/tests/suite/embedded_terminal.rs::antigravity_panes_adopt_their_conversation_title_and_preserve_manual_renames`, `crates/interface-terminal/src/antigravity_names.rs::generated_annotation_replaces_the_first_request_and_ignores_metadata`

### Antigravity, Muse and OpenCode panes reopen after they switch conversations

- **Completed:** 2026-10-21T07:37:27-05:00
- **Kind:** fix
- **Release:** v0.3.165
- **Commit:** `1ef3536` (`fix/native-provider-reopen-identity`)
- **Notes:** An Antigravity pane that started a new conversation comes back on that conversation after restarting Interface, instead of a failed tab
- **Commits:** fix(terminals): file changed Antigravity, Muse and OpenCode sessions so their panes reopen
- **Pins:** `apps/interface-host/src/bootstrap_tests.rs::changed_native_provider_session_reaches_the_saved_roster`

### Remove Halt all and update; Restart to update installs directly

- **Completed:** 2026-10-21T07:31:50-05:00
- **Kind:** feature
- **Release:** v0.3.165
- **Commit:** `7fb692e` (`feat/remove-halt-all-20261021`)
- **Notes:** Restart to update shows a plain confirm and restarts; no pane is sent a stop request
- **Commits:** Remove "Halt all and update"
- **Pins:** `apps/interface-host/src/updater.rs::restart_to_update_installs_without_a_session_checkpoint`, `electron/src/renderer/update-restart-ui.test.ts::requires explicit confirmation before mutating Rust updater state`, `crates/interface-terminal/src/service/workspace_sleep.rs::agy_osc7501_tracks_turns_and_permission_waits`, `apps/interface-host/src/terminal_tests.rs::native_reopen_resumes_the_exact_agy_and_muse_session`
- **Supersedes:** f28fecf1 — the owner removed Halt all and update on 2026-10-21; 20c5b96 — the Antigravity and Muse update pause went with Halt all and update; their exact-session reopen stays and is tested by native_reopen_resumes_the_exact_agy_and_muse_session; ad76ef6 — Antigravity indicators are unchanged; the test lost only its update-checkpoint assertions and is renamed agy_osc7501_tracks_turns_and_permission_waits

### Preview file and image attachments in-app

- **Completed:** 2026-10-20T22:41:57-05:00
- **Kind:** feature
- **Release:** v0.3.165
- **Commit:** `d1ec1bf` (`feat/in-app-attachment-preview`)
- **Notes:** Click an attached file or image in the composer or a sent message to inspect it in the side-by-side preview panel with image zoom and lightbox
- **Commits:** feat: preview file and image attachments in-app
- **Pins:** `crates/interface-workspace/src/output_preview_tests.rs::exact_file_grant_reads_only_that_attachment_outside_project_roots`, `electron/src/renderer/output-preview.test.ts::resolves in-project and external attachment preview targets`

### Speed up release packaging and update staging

- **Completed:** 2026-10-20T20:49:06-05:00
- **Kind:** speed
- **Release:** v0.3.164
- **Commit:** `b23c3d6` (`speed/promote-packaging-20261020`)
- **Notes:** Release packaging compiles with parallel codegen, caches iface lib on version bumps, signs /private/tmp apps in place, and skips the redundant DMG target
- **Pins:** `xtask/src/release.rs::selected_update_artifacts_accepts_zip_only_manifest`, `electron/scripts/sign-mac.test.cjs::path remapping adds timestamp none only to the local development build`

### Mark Muse StopFailure and Interrupt hooks idle

- **Completed:** 2026-10-20T20:12:53-05:00
- **Kind:** fix
- **Release:** v0.3.163
- **Commit:** `d36e1b7` (`fix/muse-stop-failure-activity-20261020`)
- **Notes:** A Muse Code pane that stops on a usage limit, error or interrupt returns to idle instead of staying active
- **Pins:** `crates/interface-terminal/src/native_checkpoint.rs::muse_stop_failure_and_interrupt_release_to_idle_and_ignore_post_stop_noise`

### Fix Claude worktree receipt reconciliation and unsaved shell reopen authority

- **Completed:** 2026-10-20T18:36:11-05:00
- **Kind:** fix
- **Release:** v0.3.162
- **Commit:** `6900923` (`fix/terminal-reopen-capture-and-authority`)
- **Notes:** Claude panes that used a worktree and newly spawned shells reopen cleanly after restart
- **Pins:** `apps/interface-host/src/claude_terminal_session_tests.rs::owned_capture_adopts_and_reconciles_a_worktree_clear_receipt_and_allows_reopen`, `crates/interface-desktop-services/src/service_tests.rs::in_flight_roster_save_before_filing_does_not_strip_newly_spawned_reopen_authority`

### Antigravity panes show working, waiting, and done indicators

- **Completed:** 2026-10-20T17:08:10-05:00
- **Kind:** fix
- **Release:** v0.3.161
- **Commit:** `4125d94` (`fix/agy-working-indicator-20261020`)
- **Notes:** Antigravity panes show the working indicator during turns, wait on approvals, and mark finished turns done
- **Commits:** Support Antigravity OSC 7501 working, blocked, and done status frames
- **Pins:** `electron/src/renderer/terminal-activity.test.ts::tracks working, blocked, done, error, and clear across Antigravity and shell panes`, `crates/interface-terminal/src/service/workspace_sleep.rs::agy_osc7501_tracks_turns_and_preserves_mid_turn_checkpoint_markers`

### Fix GUI review bugs, left-behind panels, and paper cuts

- **Completed:** 2026-10-20T17:01:27-05:00
- **Kind:** fix
- **Release:** v0.3.161
- **Commit:** `5a6615f` (`fix/gui-review-bugs-20261020`)
- **Notes:** Primary CTA hover/disabled contrast, From terminals inbox shelf actions and paging, Away loop composer lock, and left-behind panel polish work cleanly
- **Commits:** Fix GUI review bugs, left-behind panel inconsistencies, and paper cuts (GUI-01..GUI-18)
- **Pins:** `electron/src/renderer/thread-inbox.test.ts::pages From terminals independently of Settled`, `electron/src/renderer/away-thread.test.ts::locks the composer and turn actions while the Away loop is running`

### Cache opencode version probes, reduce ThreadHost lock/view work, and trim git and Claude scans

- **Completed:** 2026-10-20T16:51:57-05:00
- **Kind:** speed
- **Release:** v0.3.161
- **Commit:** `8c7206b` (`speed/wave2-review-20261020`)
- **Notes:** Usage polling, thread background ticks, commit review diffs, and Claude session listings run with fewer subprocesses and lock holds
- **Commits:** speed: cache opencode version probes, trim ThreadHost view/lock overhead, and cut redundant git/Claude scans
- **Pins:** `crates/engine-acp/src/opencode_usage_tests.rs::version_probe_is_cached_across_usage_reads_until_executable_changes`, `crates/interface-review/src/thread_ship_tests.rs::branch_state_resolves_branch_remote_and_default_without_scanning_working_tree`

### Keep terminal/thread props stable, gate hidden overview timers, and refresh repeated EOF keys

- **Completed:** 2026-10-20T16:33:48-05:00
- **Kind:** speed
- **Release:** v0.3.161
- **Commit:** `293071f` (`speed/wave1-review-20261020`)
- **Notes:** Switching surfaces or streaming turns keeps idle panes and hidden overviews quiet
- **Commits:** speed: keep terminal/thread props stable, gate hidden timers, refresh EOF stream keys
- **Pins:** `electron/src/renderer/app-memo-props.test.ts::keeps outputFeedbackTargets and withShip references stable when visible fields do not change`, `electron/src/main/terminal-data-transport.test.ts::refreshes an already-ended stream key without evicting it prematurely when the window rolls`

### Undo a worktree thread's turn files from Changes

- **Completed:** 2026-10-20T16:14:47-05:00
- **Kind:** feature
- **Release:** v0.3.161
- **Commit:** `1ef2083` (`feat/threads-leftovers-20261020`)
- **Notes:** In a worktree thread's Changes, a turn's Undo restores its files (recovery point kept); the conversation is unchanged.
- **Commits:** Changes panel undoes a worktree turn's files; Threads DOM lane: a worktree turn's Undo asks, then restores
- **Pins:** `electron/src/renderer/thread-view.test.ts::restores a worktree turn's files only, saying where the recovery point is`, `electron/src/renderer/components/ThreadDiffPanel.test.ts::offers Undo turn on a turn of a local or owned-worktree thread, never a shared worktree's`

### Hidden sidebar comes back on hovering its toggle beside the traffic lights

- **Completed:** 2026-10-20T16:13:02-05:00
- **Kind:** feature
- **Release:** v0.3.161
- **Commit:** `0d97b51` (`sidebar-toggle-peek`)
- **Notes:** Hide the sidebar, rest the pointer on the toggle by the traffic lights: the panel slides out below it and leaves when you move away; the left edge does nothing
- **Pins:** `electron/src/renderer/sidebar-peek.test.ts::slides the hidden sidebar out after the pointer rests on its toggle, and away once it leaves`, `electron/src/renderer/sidebar-peek.test.ts::does nothing while the sidebar is docked, so hiding it under the pointer never brings it straight back`
- **Supersedes:** 81648f1 — owner moved the reveal from the window's left edge to the sidebar toggle beside the traffic lights (2026-10-20)

### TODO: relay allowlist set, phone rebuilt

- **Completed:** 2026-10-20T15:55:16-05:00
- **Kind:** record
- **Release:** v0.3.161
- **Commit:** `388b216` (`record/phone-path-20261020`)
- **Notes:** TODO: relay allowlist set, phone rebuilt, install awaits the phone

### Dev app parses JSON about twice as fast

- **Completed:** 2026-10-20T15:39:57-05:00
- **Kind:** speed
- **Release:** v0.3.161
- **Commit:** `7beb9b7` (`speed/deps-opt-level-20261020`)
- **Notes:** Interface Dev runs optimised dependencies: JSON about twice as fast, crypto about four times. First build after this rebuilds dependencies once.
- **Commits:** Dev app parses JSON about twice as fast: optimised dependencies in debug builds; TODO: opt-level 2 landed at the owner's word

### Antigravity and Muse pause and resume for updates

- **Completed:** 2026-10-20T15:27:48-05:00
- **Kind:** feature
- **Release:** v0.3.161
- **Commit:** `2ff0a90` (`feat/update-halt-agy-muse-20261020`)
- **Notes:** New Antigravity and Muse panes find stopping points before updating, then resume the same chats once.
- **Commits:** Support Antigravity and Muse update checkpoints
- **Pins:** `apps/interface-host/src/terminal_tests.rs::native_update_resume_reopens_agy_and_muse_and_sends_one_continuation`, `crates/interface-terminal/src/muse_resume.rs::muse_resume_waits_for_exact_process_session_and_input_ready_without_a_hook`

### Installed Interface stops re-copying a staged update

- **Completed:** 2026-10-20T14:59:09-05:00
- **Kind:** fix
- **Release:** v0.3.161
- **Commit:** `3945ce0` (`fix/updater-no-recopy-20261020`)
- **Notes:** With an update staged, update.zip in ~/Library/Caches/interface-electron-updater is no longer rewritten every few minutes
- **Commits:** Stop re-copying a staged update into the differential cache
- **Pins:** `electron/src/main/native-updater.test.ts::never re-copies a staged update into the differential cache`

### Antigravity usage read no longer opens Google sign-in in Chrome

- **Completed:** 2026-10-20T14:57:39-05:00
- **Kind:** fix
- **Release:** v0.3.161
- **Commit:** `4f7c8c0` (`fix/agy-usage-browser-login-20261020`)
- **Notes:** No new Chrome tabs ask you to log into Google Antigravity; Gemini quota reads as before when signed in
- **Commits:** Stop the Antigravity usage read from opening Google sign-in in Chrome
- **Pins:** `apps/interface-host/src/antigravity_usage.rs::the_childs_browser_openers_are_no_ops`, `apps/interface-host/src/antigravity_usage.rs::a_failed_read_backs_off_instead_of_respawning`

### Hidden sidebar comes back when the pointer rests on the left edge

- **Completed:** 2026-10-20T14:52:24-05:00
- **Kind:** feature
- **Release:** v0.3.161
- **Commit:** `e6bfa12` (`sidebar-edge-peek`)
- **Notes:** Hide the sidebar, then rest the pointer on the window's left edge: it slides back over the panes and leaves when you move away; the toggle keeps it
- **Pins:** `electron/src/renderer/sidebar-peek.test.ts::slides the hidden sidebar out after the pointer rests on the left edge, and away once it leaves`

### TODO: sprintf, lead-2/lead-8 and Computer-note rechecks

- **Completed:** 2026-10-20T14:51:44-05:00
- **Kind:** record
- **Release:** v0.3.161
- **Commit:** `2297fa9` (`record/easy-items-recheck-20261020`)
- **Notes:** TODO: recheck sprintf, lead-2/lead-8 and the Computer tab note

### Remove the retired OpenCode native worker view

- **Completed:** 2026-10-20T14:43:21-05:00
- **Kind:** internal
- **Release:** v0.3.161
- **Commit:** `0cdf475` (`chore/drop-opencode-native-view-20261020`)
- **Notes:** Remove the OpenCode native-view session code; Drop the native path from the OpenCode worker runner

### Models pause for updates and resume their saved work

- **Completed:** 2026-10-20T14:40:05-05:00
- **Kind:** feature
- **Release:** v0.3.161
- **Commit:** `fc5d069` (`feat/update-halt-resume-20261020`)
- **Notes:** Halt all and update asks working models for a handoff, waits for every stop, then resumes the same sessions after restart.
- **Commits:** Add cooperative session checkpoints for app updates
- **Pins:** `crates/interface-updater/src/sessions.rs::all_models_must_checkpoint_before_install_and_failed_install_can_resume_them`, `apps/interface-host/src/terminal_tests.rs::update_resume_reopens_the_exact_saved_provider_session_with_one_continuation`

### Gate passes from an armed, busy Interface pane

- **Completed:** 2026-10-20T12:49:27-05:00
- **Kind:** fix
- **Release:** v0.3.161
- **Commit:** `929a07c` (`fix/overlay-tests-pane-env-20261020`)
- **Notes:** Running the test gate from inside Interface on a busy machine no longer fails the shell-overlay and terminal tests.
- **Commits:** Overlay tests ignore the runner's own armed pane; Parked test children outlive a loaded gate
- **Pins:** `apps/interface-host/src/bootstrap_tests.rs::manual_codex_overlay_adds_titles_and_preserves_user_overrides`, `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_pass_off_into_a_live_subdirectory_keeps_its_root_and_restarts`

### TODO: Muse Code plan in the usage tracker is deferred

- **Completed:** 2026-10-20T09:06:44-05:00
- **Kind:** record
- **Release:** v0.3.161
- **Commit:** `4959405` (`record/muse-usage-deferred`)
- **Notes:** TODO: Muse Code plan in the usage tracker is deferred

### Muse Code shows its mark on the iPhone

- **Completed:** 2026-10-20T08:58:09-05:00
- **Kind:** look
- **Release:** v0.3.160
- **Commit:** `c842b93` (`look/phone-muse-mark-20261020`)
- **Notes:** Muse Code sessions on the phone show the blue Muse squiggle instead of a terminal glyph.
- **Commits:** Show the Muse mark on the iPhone
- **Pins:** `apps/interface-ios/MacAddressTests.swift::testMuseCodeSessionsShowTheMuseMarkInMuseBlue`

### Muse Code shows in Muse blue

- **Completed:** 2026-10-20T08:47:02-05:00
- **Kind:** look
- **Release:** v0.3.160
- **Commit:** `301540f` (`look/muse-blue-20261020`)
- **Notes:** The Muse Code squiggle and its sidebar pips are blue: deep blue on light themes, brighter blue on dark.
- **Commits:** Show Muse Code in Muse blue
- **Pins:** `electron/src/renderer/components/ProviderLogo.test.ts::draws Muse Code with the official Muse squiggle in Muse blue`
- **Supersedes:** a2af762 — the owner asked for Muse blue instead of the neutral accent

### Muse Code uses its official logo

- **Completed:** 2026-10-18T09:23:58-05:00
- **Kind:** look
- **Release:** v0.3.159
- **Commit:** `7a98951` (`fix/muse-official-logo-20261018`)
- **Notes:** Muse Code shows the official Muse squiggle in the launcher and pane headers.
- **Commits:** Use the official Muse logo for Muse Code
- **Pins:** `electron/src/renderer/components/ProviderLogo.test.ts::draws Muse Code with the official Muse squiggle and keeps its accent neutral`

### TODO: the 14 Oct review work shipped in 0.3.158

- **Completed:** 2026-10-15T14:45:32-05:00
- **Kind:** record
- **Release:** v0.3.159
- **Commit:** `495a344` (`record/ship-0-3-158`)
- **Notes:** TODO: the 14 Oct review work shipped in 0.3.158

### Provider CLI drift canary: Settings says what changed in a CLI since its last check

- **Completed:** 2026-10-15T09:48:57-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `c40ed87` (`feat/gate-tooling-20261015`)
- **Notes:** After a CLI updates, Settings → Models & providers → Terminal tools says "Changed since last check: old → new"; iface doctor providers lists every CLI
- **Commits:** Provider CLI drift canary: Settings says what changed in a CLI since the last check; Provider canary: Settings never waits on a first check; host suites expect protocol 51
- **Pins:** `crates/interface-terminal/src/provider_canary_tests.rs::provider_canary_reports_a_matching_cli_and_names_each_drifted_flag_and_choice`, `crates/interface-terminal/src/provider_canary_tests.rs::provider_canary_runs_only_version_help_listing_and_schema_commands`, `apps/interface-host/src/terminal_tests.rs::settings_terminal_tools_say_what_changed_in_a_cli_since_the_last_provider_check`, `electron/src/renderer/terminal-tools.test.ts::names the versions and what no longer matches, and says nothing without a change`

### Packaged smoke stops pinning look values; the DOM gate checks the pane grid against its tokens

- **Completed:** 2026-10-15T09:27:53-05:00
- **Kind:** internal
- **Release:** v0.3.158
- **Commit:** `edbc5a7` (`feat/gate-tooling-20261015`)
- **Notes:** Packaged smoke stops pinning look values; the DOM gate checks the grid against its tokens

### Cold-start restore matrix: every saved pane reopens or says why after a restart

- **Completed:** 2026-10-15T09:19:50-05:00
- **Kind:** internal
- **Release:** v0.3.158
- **Commit:** `b3c6cfc` (`feat/gate-tooling-20261015`)
- **Notes:** Cold-start restore matrix: every saved pane reopens or says why, never blank

### Backlog: closed entries moved to done, status lines corrected, one owner decision sheet

- **Completed:** 2026-10-15T08:48:29-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `9c9acd1` (`record/backlog-triage-20261015`)
- **Notes:** Backlog: closed entries moved to done, status lines corrected, one owner decision sheet

### Review says git timed out (iCloud may be downloading), not missing; project search has a deadline

- **Completed:** 2026-10-15T08:41:56-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `170ce2e` (`feat/gate-tooling-20261015`)
- **Notes:** Review on an iCloud project whose git stalls says 'Git did not finish … within 15 seconds' and names iCloud
- **Commits:** Review says git timed out (iCloud likely) and project search is deadline-bounded
- **Pins:** `crates/interface-review/tests/suite/repository_review.rs::review_says_git_timed_out_and_names_icloud_instead_of_calling_git_missing`, `crates/interface-workspace/src/search.rs::project_search_says_a_project_still_downloading_from_icloud_instead_of_hanging`

### Second opinion and Continue with in a thread; Threads previews; plan headroom

- **Completed:** 2026-10-15T08:33:07-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `917572c` (`feat/threads-second-opinion-20261014`)
- **Notes:** Thread ⋯: Second opinion from Codex (a card above the composer) and Continue with; Preview opens a changed PDF/Markdown beside it; chips say % left
- **Commits:** Threads: Second opinion, Continue with, output preview and plan headroom (renderer); Settled looks: second opinions, Continue with, Threads preview and plan headroom, approved by the owner; Record review B4, B5 and E: the renderer lands after the owner's look approval
- **Pins:** `electron/src/renderer/components/ThreadOpinionCards.test.ts::is the other provider's card, never a turn: attributed, held until sent on`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::shows what goes out before anything leaves: Ask or Cancel, with the packet`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::offers Continue with when the thread's provider hit its limit, never by itself`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::puts Second opinion and Continue with in the pane menu, waiting while a turn runs`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::offers Preview on a changes card's previewable files`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::lets Quick Open open the highlighted PDF, image or Markdown in the preview`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::says it quietly on the model chip, in its accessible name and tooltip`, `electron/src/renderer/components/ThreadOpinionCards.test.ts::puts it under an agent tile's name in the launcher`, `electron/src/renderer/thread-opinion.test.ts::draws a thread's consults as cards: review, running, the reply, sending, failed`, `electron/src/renderer/thread-opinion.test.ts::offers Continue with the provider with the most headroom when a thread hits its limit`, `electron/src/renderer/thread-opinion.test.ts::says what is left of the fullest window, quietly, and nothing for an unknown`

### Project reads say a file is still downloading from iCloud instead of hanging

- **Completed:** 2026-10-14T23:49:45-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `c22a3e9` (`fix/security-robustness-20261014`)
- **Notes:** In an iCloud project with evicted files, Quick Open, previews, Changes and Threads say it is still downloading within seconds
- **Pins:** `crates/interface-common/src/bounded_read.rs::a_read_that_blocks_on_open_fails_at_its_deadline_and_a_quick_one_answers`, `crates/interface-review/src/thread_git_tests.rs::an_index_still_downloading_from_icloud_fails_clearly_at_its_deadline`, `crates/interface-review/tests/suite/repository_review.rs::untracked_files_still_downloading_from_icloud_are_omitted_at_their_deadline`, `crates/interface-workspace/src/search.rs::quick_open_says_a_project_still_downloading_from_icloud_instead_of_hanging`, `crates/interface-workspace/src/editor.rs::a_preview_read_still_downloading_from_icloud_says_so_at_its_deadline`

### An agent that finishes in a parked workspace gets its shell at once (Rust decides)

- **Completed:** 2026-10-14T23:36:55-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `2924551` (`fix/security-robustness-20261014`)
- **Notes:** Let Codex (or any agent) exit cleanly while its workspace is parked: its row and pane show a shell right away, not a dead tab
- **Commits:** Rust hands a cleanly exited agent's pane its shell, whatever the window shows
- **Pins:** `apps/interface-host/src/terminal_tests.rs::an_agent_that_exits_by_itself_is_handed_a_shell_by_rust_whatever_the_window_shows`, `crates/interface-terminal/src/shell_handoff.rs::only_an_agent_that_exits_cleanly_is_handed_a_shell_where_it_worked`, `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalExitHandoffFixture`, `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalHandoffSwitchFixture`, `electron/src/renderer/terminal-shell-handoff.test.ts::moves a parked workspace's saved layout without touching the shown one`, `crates/interface-contracts/src/terminal.rs::a_handed_off_shell_names_the_agent_it_replaces_like_the_shared_fixture`

### Record completed workflow acceptance and verified Dev landing

- **Completed:** 2026-10-14T23:26:55-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `342f106` (`feat/workflow-completion-20261014`)
- **Notes:** Record completed workflow acceptance and verified Dev landing

### Complete reviewed workflows across desktop, phone and background runs

- **Completed:** 2026-10-14T23:25:35-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `66d2ea9` (`feat/workflow-completion-20261014`)
- **Notes:** Choose commit hunks, reuse ready worktrees and recipes, review outputs, recover prompts, inspect tools, use the phone inbox, and run with the window closed
- **Commits:** Complete reviewed desktop, phone and background workflows
- **Pins:** `crates/interface-review/src/thread_commit_tests.rs::selected_hunk_preserves_unselected_content_and_unrelated_staging`, `crates/interface-settings/src/workflows.rs::recipes_reopen_restore_context_and_require_explicit_use_before_setup_changes`, `crates/interface-settings/src/workflows.rs::setup_reserve_survives_restart_as_interrupted_without_replay`, `crates/interface-settings/src/output_reviews.rs::durable_reviews_keep_immutable_versions_and_refuse_lost_updates`, `crates/interface-settings/src/prompt_recovery.rs::recovery_never_replays_unknown_and_retains_original_context_after_restart`, `crates/interface-workspace/src/inventory_provider_tests.rs::mcp_check_initializes_and_pings_without_tools_or_raw_output`, `crates/interface-remote/src/threads.rs::inbox_includes_only_bound_agent_work_and_keeps_completed_tasks_read_only`, `crates/interface-settings/src/background.rs::background_choice_survives_restart_and_has_one_writer`
- **Supersedes:** 3caae8e0 — replace text-only retry with receipt-bound recovery that preserves context and refuses ambiguous replay; c79f64e2 — update the retained Away integration assertion for protocol 49; loop behavior remains covered

### Grok and Antigravity get Interface's tools only when you switch them on

- **Completed:** 2026-10-14T23:12:37-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `f5d59ff` (`fix/security-robustness-20261014`)
- **Notes:** Settings → Models & providers → Terminal tools: Grok and Antigravity have an off-by-default "use Interface's tools" switch
- **Commits:** Grok and Antigravity use Interface's tools only once switched on; cargo fmt the pane tools module; Protocol 47 in the host's frame shape tests; Launch profile listing gets its own method
- **Pins:** `apps/interface-host/src/terminal_tests.rs::settings_switches_grok_onto_interfaces_tools_only_when_asked_and_never_at_start`, `apps/interface-host/src/pane_mcp.rs::pane_tools_are_written_only_when_switched_on_and_only_interfaces_entry_is_removed`, `electron/src/renderer/terminal-tools.test.ts::shows Settings' switch off until the owner turns it on, and locks a taken or unreadable one`, `crates/interface-contracts/src/terminal.rs::pane_tools_states_and_changes_match_the_shared_fixture`

### Away reviews with the other vendor by default; the start form picks the reviewer

- **Completed:** 2026-10-14T23:00:22-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `e4646a5` (`feat/away-reviewer-20261014`)
- **Notes:** Away's start form shows Reviewed by: the other vendor when it can run; each review card in the thread names the provider that reviewed
- **Pins:** `crates/interface-collaboration/src/project_away/tests.rs::a_review_runs_on_the_chosen_reviewer_with_its_own_restrictions`, `crates/interface-collaboration/src/project_away/tests.rs::a_v2_store_is_carried_over_and_its_loops_review_as_before`, `apps/interface-host/src/project_away.rs::a_review_session_starts_on_the_reviewers_provider`, `crates/interface-contracts/src/away.rs::away_reviewer_fixture_round_trips_and_defaults_to_the_worker`, `electron/src/renderer/away-view.test.ts::defaults to the other vendor when it can run, else the worker reviews`

### Agents ask before every web fetch

- **Completed:** 2026-10-14T22:46:51-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `12783e4` (`fix/security-robustness-20261014`)
- **Notes:** An agent in Ask mode asks you before any WebFetch (GET included); Away still never fetches; web search stays automatic
- **Commits:** Agents ask before every WebFetch
- **Pins:** `crates/interface-desktop-services/src/agent_policy.rs::web_fetch_asks_the_owner_first_whatever_the_method_or_task`

### Threads avoid redundant reads and clean up background work

- **Completed:** 2026-10-14T22:41:10-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `1801448` (`fix/performance-review-20261014`)
- **Notes:** Threads retain the newest diff during rapid selections, stop queued reads when closed, and reuse unchanged thread rows.
- **Commits:** Reduce redundant Threads work and fix read lifecycle cleanup
- **Pins:** `electron/src/renderer/latest-read.test.ts::a hundred selections run only the in-flight read and the newest pending read`, `electron/src/renderer/use-plan-usage.test.ts::disposal ignores a late answer and prevents its queued CLI read`, `crates/interface-settings/src/threads_tests.rs::view_checks_each_shared_worktree_once_and_refreshes_on_the_next_read`, `crates/interface-settings/src/threads_tests.rs::seeding_a_full_library_keeps_newest_first_across_order_key_spreads`

### The phone shows what needs you and the Mac's Threads

- **Completed:** 2026-10-14T22:36:15-05:00
- **Kind:** look
- **Release:** v0.3.158
- **Commit:** `e4ecd56` (`feat/phone-needs-you-look-20261014`)
- **Notes:** On the iPhone, a session waiting for permission says Needs you and sorts first, and Code | Threads opens the Mac's Threads with Allow once and a follow-up field
- **Commits:** Settled look: the phone shows what needs you and the Mac's Threads
- **Pins:** `apps/interface-ios/MacAddressTests.swift::testSessionsThatNeedYouComeFirstAndLiftTheirProject`, `apps/interface-ios/MacAddressTests.swift::testAnUncertainFollowUpWaitsForANewerTurnOrTheOwnersCheck`

### Record review D6: host and protocol landed, phone screens await look approval

- **Completed:** 2026-10-14T22:27:35-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `edf8b86` (`record/phone-needs-you-20261014`)
- **Notes:** Record review D6: host and protocol landed, the phone screens await look approval

### Record review B4, B5 and E: the backend landed, the renderer awaits look approval

- **Completed:** 2026-10-14T22:20:50-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `44f872d` (`docs/threads-b4-record-20261014`)
- **Notes:** Record review B4, B5 and E: the backend landed, the renderer awaits look approval

### The phone's Mac answers which sessions need you and serves Threads (phone screens await approval)

- **Completed:** 2026-10-14T22:17:23-05:00
- **Kind:** internal
- **Release:** v0.3.158
- **Commit:** `b2e9672` (`feat/phone-needs-you-20261014`)
- **Notes:** The phone's Mac answers what needs you and serves Threads
- **Pins:** `crates/interface-remote/src/threads.rs::the_phone_lists_threads_that_need_you_then_working_then_the_macs_order`, `crates/interface-remote/src/workspaces.rs::only_roster_v4_says_which_sessions_need_you`, `crates/interface-remote/src/http_threads_tests.rs::an_approval_answer_needs_the_lease_runs_once_and_never_repeats_an_unknown`, `crates/interface-remote/src/http_threads_tests.rs::a_follow_up_is_sent_once_under_the_lease_and_never_mistaken_for_an_answer`, `apps/interface-host/src/remote.rs::the_phone_answers_only_the_current_request_no_pane_holds`, `apps/interface-ios/MacAddressTests.swift::testThreadsUseTheSharedFixtureAndAddressOnlyWellFormedKeys`

### A consult never writes to a session a Code pane holds; Second opinion and Continue with on the wire

- **Completed:** 2026-10-14T22:09:48-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `4d2eff8` (`feat/threads-b4-wire-20261014`)
- **Notes:** From the Coordination canvas, a consult on a thread that is open in a Code pane is refused and nothing reaches that session
- **Commits:** Threads: Second opinion and Continue with on the wire; a consult never writes to a pane's session
- **Pins:** `apps/interface-host/src/collaboration_tests.rs::a_consult_never_writes_to_a_session_a_pane_holds`, `apps/interface-host/src/threads_tests.rs::second_opinion_resumes_the_thread_and_submits_one_consult_on_its_latest_turn`, `apps/interface-host/src/threads_tests.rs::second_opinion_refuses_a_held_session_a_running_turn_and_its_own_provider`, `apps/interface-host/src/threads_tests.rs::continue_with_prepares_the_threads_folder_and_binds_a_handoff_lineage`, `crates/interface-contracts/src/threads.rs::shared_v46_second_opinion_and_handoff_round_trip_and_reject_extra_fields`, `crates/interface-settings/src/threads_tests.rs::a_v2_store_is_carried_over_once_and_wins_over_v1`, `crates/interface-settings/src/threads_tests.rs::a_handoff_bind_records_its_lineage_and_only_v3_holds_one`, `crates/interface-collaboration/src/thread_handoff.rs::handoff_prompt_carries_the_changes_and_newest_conversation_inside_the_packet_bound`
- **Supersedes:** df9def16 — store v3 (protocol 44): a_v1_store_is_carried_over_once_and_left_unchanged now expects the v1 store carried to threads.v3.json as version 3

### An Away loop's thread wears Away's coffee in the Threads inbox

- **Completed:** 2026-10-14T22:07:57-05:00
- **Kind:** look
- **Release:** v0.3.158
- **Commit:** `4331a8c` (`feat/away-inbox-label-20261014`)
- **Notes:** Threads inbox: a row an Away loop runs as shows a small coffee glyph before its title
- **Commits:** Threads inbox: an Away loop's thread carries Away's coffee before its title; Threads inbox tests read the protocol 45 fixture; Settled look: an Away loop's inbox row wears Away's coffee before its title
- **Pins:** `electron/src/renderer/components/ThreadInbox.test.ts::carries Away's coffee before the title and says Away loop`, `electron/src/renderer/thread-inbox.test.ts::marks the row of a thread an Away loop runs as, and names it so`

### Record Away loops as Threads (B3) and the inbox look awaiting approval

- **Completed:** 2026-10-14T22:02:34-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `ba4a6a0` (`docs/away-threads-todo-20261014`)
- **Notes:** Record Away loops as Threads (review B3) and its pending inbox look

### Away loops run as Threads: their own worktree, a turn per round, the review's verdict, the report

- **Completed:** 2026-10-14T21:59:10-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `d731888` (`feat/away-threads-loop-20261014`)
- **Notes:** Start Away: its thread opens in a new worktree, each round a turn with its diff and the review's verdict, the report at the end; the panel lists loops
- **Commits:** Away runs as a thread: contracts, store v2 and the controller (in progress); Away runs as a thread: host hold, view join, protocol 44, panel and thread cards (in progress); Away runs as a thread: gate fixes, vetted worktree, real-host loop in its own worktree; Away: a started loop's thread opens from the Threads push; Away real-host test: the thread read helper takes its payload as the request helper does; Away threads: the loop's prompts show live, the working round has its line, the fake answers rounds; Threads spec: an Away loop's prompts reach its pane as live user messages
- **Pins:** `crates/interface-collaboration/src/project_away/tests.rs::a_loop_runs_as_one_thread_in_its_own_worktree_with_fresh_reviews`, `apps/interface-host/src/threads_tests.rs::an_away_loop_holds_its_thread_and_takes_only_its_own_prompts`, `apps/interface-host/tests/suite/project_away.rs::project_away_real_host_native_turn_review_and_retained_results`, `crates/interface-collaboration/src/project_away/tests.rs::a_v1_store_is_carried_over_once_and_left_unchanged`, `electron/src/renderer/away-thread.test.ts::puts each round's review under its turn, and the report at the end`, `crates/interface-contracts/src/threads.rs::shared_v45_away_thread_round_trips_and_rejects_extra_fields`, `apps/interface-host/src/dispatcher_tests.rs::a_host_prompt_is_shown_in_its_pane_before_the_send`

### Rust decides which terminal panes need you (desktop reads it; the phone can next)

- **Completed:** 2026-10-14T21:49:38-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `a272274` (`feat/phone-needs-you-20261014`)
- **Notes:** A pane asking for permission shows Needs attention as before, and still does after reloading the window
- **Commits:** Rust decides which terminal panes need you
- **Pins:** `crates/interface-terminal/src/attention.rs::terminal_attention_follows_the_shared_fixture`, `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_bell_makes_the_pane_need_you_until_a_keystroke_answers_it`, `electron/src/renderer/terminal-attention.test.ts::publishes the host's waiting and never its own`

### The launcher folds missing agents under More, with install hints

- **Completed:** 2026-10-14T21:17:33-05:00
- **Kind:** look
- **Release:** v0.3.158
- **Commit:** `6aa54a3` (`feat/launcher-fold-20261014`)
- **Notes:** New workspace: missing agents other than Claude Code, Codex and Terminal fold under More with install hints and Refresh; Settings shows commands as code
- **Commits:** The launcher folds missing agents under More, each with its install hint; Settled look: the workspace launcher folds missing agents under More; Settings' Terminal tools set a missing tool's install commands as code; Record review C4 part 2 as landed
- **Pins:** `electron/src/renderer/components/TerminalLauncher.test.ts::folds tools that are not installed under More, each with its install hint`, `electron/src/renderer/components/SettingsPage.test.ts::shows a missing tool's install hint alone, its commands set as code`

### Live panes and a From terminals shelf in Threads

- **Completed:** 2026-10-14T21:14:40-05:00
- **Kind:** look
- **Release:** v0.3.158
- **Commit:** `2d12e0b` (`feat/one-product-looks2-20261014`)
- **Notes:** Code's pane-card column and its ··· row say Live panes (tabs Panes, Agent tasks); Threads ends in a folded From terminals shelf
- **Commits:** Live panes: the Code column of pane cards stops calling itself Agent overview; Threads inbox: a folded From terminals shelf for Code pane and CLI sessions; Threads: terminal sessions are never counted as hidden while the inbox lists them; Settled looks: Live panes and the From terminals shelf, approved by the owner
- **Pins:** `electron/src/renderer/thread-inbox.test.ts::draws Rust's From terminals as the last shelf, folded, offering Continue as thread`, `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps the reference layout rows, now inside the window's ··· menu`, `electron/src/renderer/palette-commands.test.ts::reaches Away, voice, iPhone access, Files, Browser, overview, New project and themes (review C3)`

### A thread whose provider fails at once shows Failed instead of thinking forever

- **Completed:** 2026-10-14T19:58:14-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `4f07365` (`feat/away-threads-20261014`)
- **Notes:** Start a thread that fails at once (a signed-out provider): its turn shows Failed with Sign in, and no Thinking clock keeps running
- **Commits:** Threads: a launch the provider fails within milliseconds ends turn 0 failed
- **Pins:** `apps/interface-host/src/threads_tests.rs::a_launch_that_fails_before_its_bind_ends_turn_zero_failed`

### Record review C4: Terminal tools landed, the folded launcher awaits approval

- **Completed:** 2026-10-14T19:48:41-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `c7a7f12` (`docs/c4-todo-20261014`)
- **Notes:** Record review C4: Terminal tools landed, the folded launcher awaits approval

### Record the one-product work and its pending look approval

- **Completed:** 2026-10-14T19:37:47-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `80a7b1f` (`chore/one-product-todo-20261014`)
- **Notes:** Record the one-product work (C1, C2, D8) and its pending look approval

### Models & providers lists the launcher's terminal tools, with versions and install hints

- **Completed:** 2026-10-14T19:37:24-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `3b77587` (`feat/provider-tools-20261014`)
- **Notes:** Settings → Models & providers shows Terminal tools: each launcher tool found or not installed, its version, sign-in state and install hint
- **Pins:** `apps/interface-host/src/terminal_tests.rs::settings_terminal_tools_show_each_found_cli_version_and_an_install_hint_for_the_rest`, `electron/src/renderer/terminal-tools.test.ts::lists every launcher tool with its version, sign-in state and install hint`, `crates/interface-contracts/src/terminal.rs::terminal_tool_profiles_match_the_shared_fixture`

### Threads, Code and Agent share sessions: Open in terminal, Continue as thread, Hand to teammate

- **Completed:** 2026-10-14T19:30:06-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `c6e84fa` (`feat/one-product-c1-20261014`)
- **Notes:** A thread's menu offers Open in terminal and Hand to teammate; a Claude/Codex pane's ··· offers Continue as thread; one session never has two writers
- **Commits:** WIP: Threads ↔ Code bridges, Rust side (protocol 41); Threads ↔ Code ↔ Agent bridges: Open in terminal, Continue as thread, Hand to teammate (protocol 41); Session claims: contains, as clippy asks
- **Pins:** `apps/interface-host/src/dispatcher_tests.rs::a_session_a_code_pane_drives_is_never_resumed_or_sent_to_from_here`, `apps/interface-host/src/threads_tests.rs::open_in_terminal_closes_the_thread_first_and_holds_it_for_one_pane`, `apps/interface-host/src/threads_tests.rs::continue_from_terminal_waits_for_the_pane_then_brings_the_session_into_threads`, `crates/interface-settings/src/threads_tests.rs::terminal_sessions_sit_under_from_terminals_until_brought_into_threads`, `electron/src/renderer/components/TerminalPrimitives.test.ts::offers Continue as thread on an agent pane's ··· menu, and says why when it cannot (review C1)`, `electron/src/renderer/components/AgentTile.test.ts::offers Open in terminal after the split, refused in words mid-turn (review C1)`, `electron/src/renderer/thread-inbox.test.ts::hands a teammate the thread's title, where it runs and how to find it, never its session`

### Signed-out providers say so and offer Sign in; Refresh finds a CLI installed since start

- **Completed:** 2026-10-14T19:11:13-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `0a8d4d9` (`feat/diagnostics-20261014`)
- **Notes:** Settings shows Signed out with Sign in (opens the CLI's login in Code); a signed-out turn offers Sign in; Refresh finds a newly installed CLI
- **Commits:** Record the diagnostics and recovery work, its owner checks and the instant-failure turn race; Name the error event's new cause in two pinned failed-turn tests
- **Pins:** `electron/src/renderer/settings.test.ts::shows a provider its own CLI says is signed out as Signed out, with Sign in where it has one`, `crates/engine-claude/src/mapper.rs::a_signed_out_claude_turn_fails_with_the_sign_in_cause`, `crates/interface-terminal/src/service_tests.rs::a_sign_in_pane_runs_the_cli_login_and_a_refreshed_catalog_serves_later_launches`, `apps/interface-host/src/dispatcher_tests.rs::an_explicit_provider_refresh_finds_the_clis_again_before_it_probes`, `apps/iface/src/doctor.rs::doctor_reports_needs_auth_for_a_signed_out_cli`, `electron/src/shared/providers-v41.test.ts::offers Sign in on a turn that failed because the CLI is signed out, and only there`
- **Supersedes:** 80b97b7 — mechanical only: the error event's new optional cause appears in the tests' literal and pattern; the failed-turn behavior they pin is unchanged

### One word per idea: panes, teammate conversations, Local checkout / New worktree

- **Completed:** 2026-10-14T18:59:51-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `b6398d2` (`feat/one-product-20261014`)
- **Notes:** Palette: Next pane; a teammate's tab: Conversation; launcher and Agent composer: Local checkout / New worktree; teammate New task: no access picker
- **Commits:** One word per idea: panes, conversations, Local checkout / New worktree; Agent workspace fixture and formatting follow the conversation wording; Tests follow the conversation and New worktree wording
- **Pins:** `electron/src/renderer/components/AgentWorkspacePanel.test.ts::mounts the current conversation with New conversation and past conversations`, `electron/src/renderer/composer.test.ts::a teammate's task card has no access mode or session options: its own mode decides (review C2)`, `electron/src/renderer/components/TerminalLauncher.test.ts::renders only the owner's launch roster with compact provider rows`, `electron/src/renderer/workspace-ui.test.ts::keeps command-palette actions concise and explains every known disabled state`

### New project shows its Projects folder, with Change… to pick another

- **Completed:** 2026-10-14T18:48:57-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `e69ecc4` (`feat/one-product-20261014`)
- **Notes:** New project… names the folder it will create in (~/Developer/Projects by default); Change… picks another and New project keeps using it
- **Commits:** Projects folder setting for New project (Rust store and wire request); New project shows the Projects folder with Change… (protocol 40); Format the Projects folder change; Projects folder store follows the set-aside artifact API; Host suite expects protocol 40
- **Pins:** `crates/interface-desktop-services/src/service_tests.rs::new_project_creates_in_the_chosen_projects_folder_and_reads_it_back`, `crates/interface-desktop-services/src/service_tests.rs::projects_folder_defaults_to_developer_projects_then_the_last_picked_parent`, `electron/src/renderer/components/CreateProjectDialog.test.ts::offers Change… beside the path only when the folder can be changed`

### Help menu: Reveal Logs and Copy Diagnostics; host.log; a one-time notice when a store is set aside

- **Completed:** 2026-10-14T18:35:38-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `5bfda83` (`feat/diagnostics-20261014`)
- **Notes:** Help → Copy Diagnostics copies a filtered report; Reveal Logs shows host.log; an unreadable threads file shows a one-time notice with Reveal
- **Commits:** Logs and diagnostics a user can reach; one-time notice for a set-aside store
- **Pins:** `apps/interface-host/src/diagnostics_tests.rs::a_set_aside_store_is_noticed_once_until_dismissed_and_reveals_its_file`, `crates/interface-review/src/diagnostics.rs::copy_diagnostics_names_the_build_and_providers_and_never_carries_tokens`, `apps/interface-host/src/host_log.rs::host_log_keeps_warnings_and_errors_but_not_info`, `electron/src/main/app-menu.test.ts::help offers Reveal Logs and Copy Diagnostics, and Settings… opens Settings`

### Pane menu drops the dead Show delegated work; delegation UI archived

- **Completed:** 2026-10-14T18:31:47-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `7c2cc3b` (`chore/archive-delegation-ui-20261014`)
- **Notes:** A terminal pane's ··· menu has no Show delegated work, and panes stop polling for workers
- **Commits:** Remove pane delegation from the renderer; retire its settled look; Retire the settled look for delegated workers, with the owner's approval
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::draws each pane in a plain frame, with no delegated-work item and no worker poll`

### Record the dormant-feature archive and its pending approval

- **Completed:** 2026-10-14T18:27:24-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `4535d09` (`chore/archive-dormant-20261014`)
- **Notes:** Record the dormant-feature archive and its pending renderer approval

### Control assistant, Tailscale phone route and pane delegation (host side) archived

- **Completed:** 2026-10-14T18:24:30-05:00
- **Kind:** internal
- **Release:** v0.3.158
- **Commit:** `c871c4d` (`chore/archive-dormant-20261014`)
- **Notes:** Retire and archive the Control assistant; Retire and archive the Tailscale phone route; Retire pane delegation from the host and crates; archive its docs; Record the pinned tests the three archived features replace
- **Supersedes:** 06483d55 — the Control assistant and its store were archived (owner, 2026-10-14); 2f2e1bdd — the Tailscale route was retired at the owner's request (2026-10-14); routes are nearby-only; 37049fb1 — the nearby listener stops admitting the Tailscale 100.64/10 range with the retired route; d3f0732a — the pane MCP catalogue no longer has a delegation switch to pass (pane delegation archived, 2026-10-14)

### Threads ship from the Changes panel: commit, branch, push and PR, checks, review comments, merge

- **Completed:** 2026-10-14T18:02:30-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `0cb4fc8` (`feat/threads-ship-ui-20261014`)
- **Notes:** In a project thread, the Changes panel commits the thread's files, opens a PR, fixes failing checks and merges; the card offers the next step
- **Commits:** Threads: ship controls in the Changes panel and the latest changes card; Settled looks: Threads ship foot and the changes card's ship row; Threads: a pull request Interface opened is tracked whatever the setting
- **Pins:** `electron/src/renderer/thread-ship.test.ts::runs one effect at a time and never retries an unknown outcome until a read settles it`, `apps/interface-host/src/threads_ship_tests.rs::a_local_thread_commits_its_own_files_opens_one_pull_request_and_merges_to_settle`, `crates/interface-settings/src/threads_tests.rs::a_pull_request_interface_opened_is_tracked_with_tracking_off`

### Threads find gh in ~/.local/bin too, so PR tracking works there; stable branch suggestion

- **Completed:** 2026-10-14T17:53:56-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `9b74c0c` (`feat/threads-ship-20261014`)
- **Notes:** With gh installed in ~/.local/bin, a worktree thread's PR chip appears in Threads
- **Commits:** Threads: find gh in user installs; offer one branch name per thread; Record the Threads ship work and its approval boundary in TODO
- **Pins:** `crates/interface-review/src/thread_ship_tests.rs::gh_is_found_on_path_then_in_homebrew_and_user_folders`, `crates/interface-review/src/thread_ship_tests.rs::create_branch_switches_carrying_changes_and_refuses_an_existing_name`

### Teammate New task runs behind the agent's gate: home, MEMORY.md, folders, Ask mode

- **Completed:** 2026-10-14T17:41:57-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `3f509ef` (`fix/review-quick-20261014`)
- **Notes:** A teammate's New task in a project folder appears under the agent and in Runs; Ask-mode approvals go through its policy; home or ~ is refused
- **Commits:** Teammate New task runs behind the agent's own gate (protocol 37); Teammate task start moves to protocol 38 (dev took 37 for Threads shipping); rustfmt the teammate task start; Clippy: split the task start's checks; clone_into for launch choices
- **Pins:** `apps/interface-host/src/agents_tests.rs::a_teammate_task_launches_behind_the_gate_with_home_folders_and_job`, `apps/interface-host/src/agents_tests.rs::a_teammate_task_is_refused_before_the_provider_and_leaves_no_run`, `crates/interface-settings/src/agents/tests.rs::a_one_off_task_is_admitted_once_as_a_starting_gated_run`, `crates/interface-contracts/src/agents.rs::v38_agent_task_start_round_trips_and_rejects_unknown_fields`, `electron/src/renderer/agent-profiles.test.ts::sends only the owner's words, folder and launch choices for Rust to gate`

### Threads ship commands on the wire: commit, branch, push and PR, checks, review comments, merge

- **Completed:** 2026-10-14T17:28:15-05:00
- **Kind:** internal
- **Release:** v0.3.158
- **Commit:** `37b1e78` (`feat/threads-ship-20261014`)
- **Notes:** Threads: ship a thread's work over the wire (protocol 37)

### Notification settings; native sound respects Focus; Dock badge counts waiting panes

- **Completed:** 2026-10-14T17:22:03-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `c587210` (`fix/review-quick-20261014`)
- **Notes:** Settings › Notifications has three switches; no chime when notifications are refused; the Dock badge counts terminal panes waiting for you
- **Pins:** `electron/src/renderer/notify.test.ts::uses the notification's own sound, so Focus and permission decide, and no chime (review D5)`, `electron/src/renderer/components/SettingsPage.test.ts::has a Notifications page with sound, finished and needs-input switches (review D5)`, `electron/src/renderer/dock-attention.test.ts::counts live panes waiting for input, the ones the attention tray lists`, `electron/src/shared/dock-attention.test.ts::adds waiting terminal panes to the waiting sessions`

### Palette and ··· menu reach every surface: Away, voice, iPhone, Files, Browser, themes

- **Completed:** 2026-10-14T17:14:30-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `a215090` (`fix/review-quick-20261014`)
- **Notes:** ⌘⇧P lists Away loop…, iPhone remote access, Files, Browser, Agent overview, New project and themes; ··· lists Coordination canvas, Worktrees, Context inventory
- **Commits:** Palette and ··· menu reach every surface
- **Pins:** `electron/src/renderer/palette-commands.test.ts::reaches Away, voice, iPhone access, Files, Browser, overview, New project and themes (review C3)`, `electron/src/renderer/palette-commands.test.ts::drops Integrated terminals, which only repeated Go to Code`, `electron/src/renderer/components/AttentionTray.test.ts::reaches the Coordination canvas, Worktrees and the Context inventory from ···`

### Interface keeps the Mac awake while agents, threads, teammate runs or Away loops work

- **Completed:** 2026-10-14T17:08:58-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `9d8dd46` (`fix/keep-awake-20261014`)
- **Notes:** While an agent works, pmset -g assertions lists Interface's PreventUserIdleSystemSleep; it goes once the agent is idle or only waiting
- **Commits:** Keep the Mac awake while agents work
- **Pins:** `electron/src/main/keep-awake.test.ts::holds one app-suspension blocker from the first working report and releases it when idle`, `electron/src/main/keep-awake.test.ts::lets the Mac sleep when everything is only waiting for an answer`, `electron/src/main/keep-awake.test.ts::releases when the report goes away and for good on quit`, `electron/src/main/main-wiring.test.ts::keeps the Mac awake from the work-in-flight report and releases it on quit`, `electron/src/renderer/work-in-flight.test.ts::counts teammate runs in progress, once, and what only waits`, `electron/src/shared/quit-guard.test.ts::is working while anything progresses, not while everything only waits`

### Review fixes: Task details reads plainly, ACP engines off for new agents, Relaunch after a crash

- **Completed:** 2026-10-14T17:05:39-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `15657df` (`fix/review-quick-20261014`)
- **Notes:** Palette has one Task details… row with readable inventory; New agent shows Grok/Qwen disabled with the reason; a backend-stopped alert offers Relaunch
- **Commits:** Review fixes: Task details reads plainly, ACP engines disabled for new agents, Relaunch after a crash
- **Pins:** `electron/src/renderer/components/TaskInspector.test.ts::prints the context inventory as named groups instead of raw JSON`, `electron/src/renderer/palette-commands.test.ts::offers one Task details row instead of four rows that opened the same dialog`, `electron/src/renderer/components/AgentMenu.test.ts::never creates an agent on a provider that cannot be a teammate (review A6)`, `electron/src/main/crash-dialog.test.ts::makes Relaunch the default answer when the backend stops`, `electron/src/renderer/settings.test.ts::reads a store that still holds the retired editor settings and drops them`

### Browser tab in the Agents column; Browser panes match its look

- **Completed:** 2026-10-14T17:03:46-05:00
- **Kind:** feature
- **Release:** v0.3.158
- **Commit:** `d266642` (`feat/browser-sidebar-look-20261014`)
- **Notes:** The Agents column has a Browser tab with a working browser, and Browser panes use the column's rounded search field and page card
- **Commits:** Browser pane drawn like the agent overview; Browser tab in the overview
- **Pins:** `electron/src/renderer/agent-overview.test.ts::offers a Browser tab that shows its body in place of the agents`

### TODO: review A1-A5 landing and what was not seen on screen

- **Completed:** 2026-10-14T17:01:53-05:00
- **Kind:** record
- **Release:** v0.3.158
- **Commit:** `ff91056` (`fix/review-a1-a5-20261014`)
- **Notes:** TODO: record the review A1-A5 landing and what was not seen on screen

### Review fixes: Away follows focus, Quit asks first, Start again, provider alert, ready models first

- **Completed:** 2026-10-14T17:01:13-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `a4ee5d7` (`fix/review-a1-a5-20261014`)
- **Notes:** Away opens on the focused thread's project; Cmd+Q asks while agents work; crashed panes offer Start again; provider alert opens Models & providers
- **Commits:** Fix the review's one-click traps: Away target, quit guard, Start again, provider alert, model order; Away fixture: the first start in a project is confirmed in the form
- **Pins:** `electron/src/renderer/away-view.test.ts::works on the project in front: the focused thread on Threads, the terminal on Code`, `electron/src/renderer/away-view.test.ts::opens the running loops instead of starting another`, `electron/src/renderer/away-view.test.ts::asks before the first start in a project, then starts at once`, `electron/src/shared/quit-guard.test.ts::asks how many agents are still working and names what stops`, `electron/src/main/main-wiring.test.ts::asks before a quit would end working agents`, `electron/src/renderer/terminal-shell-handoff.test.ts::offers a restart on every agent end the hand-off leaves alone`, `electron/src/renderer/components/TerminalPrimitives.test.ts::offers Start again on an ended agent pane, above xterm's own layers`, `electron/src/renderer/components/SettingsPage.test.ts::opens on the section it is asked for`, `electron/src/renderer/settings.test.ts::says what to do about a provider probe failure, keeping the CLI's own words`, `electron/src/renderer/composer-model.test.ts::lists unavailable providers after the ready ones, each in registration order`, `electron/src/renderer/composer-model.test.ts::opens a fresh card on a ready provider's model, never on Choose model`

### Threads turns keep their end and own files; durations survive restarts; shared Review is scoped

- **Completed:** 2026-10-14T16:57:01-05:00
- **Kind:** fix
- **Release:** v0.3.158
- **Commit:** `0576c7e` (`feat/threads-ship-20261014`)
- **Notes:** After a Dev restart earlier Threads turns still read 'Worked for …'; with two threads in one Local checkout a turn's Review shows only that turn's files
- **Commits:** Threads: turns keep their end and their own files (store v2, protocol 36)
- **Pins:** `apps/interface-host/src/threads_tests.rs::a_shared_checkout_turn_keeps_its_end_and_files_and_its_review_diff_covers_only_them`, `crates/interface-settings/src/threads_tests.rs::a_v1_store_is_carried_over_once_and_left_unchanged`, `electron/src/renderer/thread-transcript.test.ts::times an earlier turn from its send to its own end, after the next send and a restart`

### Muse Code is a terminal option

- **Completed:** 2026-10-12T13:05:21-05:00
- **Kind:** feature
- **Release:** v0.3.157
- **Commit:** `20f5e05` (`muse-code-profile`)
- **Notes:** New-terminal launcher and + menu list Muse Code with an M mark; choosing it runs muse in the picked folder (marked unavailable until Muse Code is installed)
- **Commits:** Add Muse Code as a terminal launch profile
- **Pins:** `crates/interface-terminal/src/launch_tests.rs::muse_launches_directly_and_refuses_resume_and_pass_off_bindings`, `electron/src/renderer/components/TerminalLauncher.test.ts::offers Muse Code with its own mark beside the other agents`, `electron/src/renderer/components/TerminalPrimitives.test.ts::leaves Muse Code out of Pass off because its TUI takes no initial prompt`

### Threads: measure a 300-turn transcript (no windowing needed)

- **Completed:** 2026-10-09T13:59:02-05:00
- **Kind:** internal
- **Release:** v0.3.157
- **Commit:** `9bda789` (`test/threads-long-transcript-20261009`)
- **Notes:** Threads: measure a 300-turn transcript in the threads DOM lane

### Threads: Retry a failed turn

- **Completed:** 2026-10-09T13:55:09-05:00
- **Kind:** feature
- **Release:** v0.3.157
- **Commit:** `b993eb7` (`feat/threads-retry-turn-20261009`)
- **Notes:** When a thread's latest turn fails, Retry sits under it; one click sends the same message again in that thread (text only), once
- **Pins:** `electron/src/renderer/thread-turns.test.ts::offers Retry on the newest turn when Rust recorded it failed`, `electron/src/renderer/thread-turns.test.ts::never offers an older failure once a later turn was sent`

### Threads: Changes panel can show a diff side by side (Stacked / Split)

- **Completed:** 2026-10-09T13:47:36-05:00
- **Kind:** feature
- **Release:** v0.3.157
- **Commit:** `90d3da6` (`feat/threads-split-diff-land-20261009`)
- **Notes:** Open Changes in a wide window and press Split: old file left, new file right, line comments still land in the prompt; Stacked stays the default
- **Commits:** Threads Changes panel can show a diff side by side (Stacked / Split)
- **Pins:** `electron/src/renderer/components/ThreadDiffPanel.test.ts::pairs three deletions with one addition row by row, then fills the right`, `electron/src/renderer/components/ThreadDiffPanel.test.ts::draws Split only when it is chosen and the panel is wide enough`

### Threads: Undo turn in the Changes panel for the turn it shows

- **Completed:** 2026-10-09T13:45:37-05:00
- **Kind:** feature
- **Release:** v0.3.157
- **Commit:** `b8ebda0` (`feat/threads-panel-undo-20261009`)
- **Notes:** In a local thread, open Changes on one turn: Undo turn in the panel header restores files to before that message, after the same confirm
- **Commits:** Threads: the Changes panel offers Undo turn for the turn it shows
- **Pins:** `electron/src/renderer/thread-turns.test.ts::finds the restore point on the row the pane matched the turn to`

### Threads keep their permission mode (Auto, Plan…) after Interface restarts

- **Completed:** 2026-10-07T15:12:09-05:00
- **Kind:** fix
- **Release:** v0.3.156
- **Commit:** `64ee194` (`fix/resume-keeps-access-20261007`)
- **Notes:** Start a thread in Auto, quit and reopen Interface, then send a follow-up: it stays in Auto and runs tools without asking.
- **Commits:** Threads keep their permission mode across restarts
- **Pins:** `apps/interface-host/src/dispatcher_tests.rs::a_session_resumes_after_a_restart_in_the_mode_its_owner_chose`

### Threads live turn: calmer — no composer arc, a still head line, steady step words

- **Completed:** 2026-10-07T15:00:49-05:00
- **Kind:** look
- **Release:** v0.3.156
- **Commit:** `482c3e4` (`feat/calm-live-turn-20261007`)
- **Notes:** Run a Threads turn: only the Wave and the step's ring move; the composer edge and head line stay still, step words don't shimmer
- **Commits:** Threads live turn: calm the frame — no composer arc, a still head line, steady step words
- **Pins:** `electron/src/renderer/components/ThreadComposer.test.ts::draws nothing that moves on the box's edge (owner: less motion)`, `electron/src/renderer/thread-view.test.ts::says the newest running step under the prompt, verb then project-relative target`
- **Supersedes:** 48ff6fd — the step's verb no longer shimmers; the owner asked for less motion

### TODO: the Threads live turn shipped in 0.3.155

- **Completed:** 2026-10-07T11:09:08-05:00
- **Kind:** record
- **Release:** v0.3.156
- **Commit:** `3275f58` (`record/todo-0155`)
- **Notes:** TODO: the Threads live turn shipped in 0.3.155

### TODO: the Threads live turn is on Dev

- **Completed:** 2026-10-07T10:59:49-05:00
- **Kind:** record
- **Release:** v0.3.155
- **Commit:** `5c219bf` (`design/threads-running-20261007`)
- **Notes:** TODO: the Threads live turn is on Dev

### Threads live turn: the run draws under its prompt, approvals dock, the inbox shows live steps

- **Completed:** 2026-10-07T10:59:18-05:00
- **Kind:** look
- **Release:** v0.3.155
- **Commit:** `21c38df` (`design/threads-running-20261007`)
- **Notes:** Start a Threads turn: steps trail under your prompt with one clock, fold into Worked for…, and an approval docks above the composer
- **Commits:** Threads: streamed words settle in, and the caret is a hued bar; Threads inbox under load: Wave and clock for working, one filled Needs you pill, a live second line; Threads: the streaming caret follows the last painted word; Threads live turn: resume entry; Threads live turn: the run draws itself under its prompt; Threads live turn: dock the request above the composer, live composer and head; Threads composer fixture: finish the 0.01ms transition before reading Send's fill; Threads live turn: an edit's peek shows the lines it adds, not the ones it keeps; Threads: the transcript region clips, so a running turn no longer slides the pane up its tile; Threads live turn: settled looks, renderer notes and resume entry
- **Pins:** `electron/src/renderer/thread-view.test.ts::says the newest running step under the prompt, verb then project-relative target`, `electron/src/renderer/thread-view.test.ts::docks a waiting approval out of the transcript: the step waits, no card`, `electron/src/renderer/thread-view.test.ts::keeps a finished turn's time once the next turn is sent`, `electron/src/renderer/thread-view.test.ts::clips the transcript's region, so nothing inside it can scroll the tile`, `electron/src/renderer/components/ThreadInbox.test.ts::says on line two what a live thread is doing, asking or failed on, in the project line's place`

### Two threads in one checkout: each turn counts only its own files

- **Completed:** 2026-10-07T10:56:50-05:00
- **Kind:** fix
- **Release:** v0.3.155
- **Commit:** `3149e14` (`feat/liveturn-s5-difffix`)
- **Notes:** Run two Local threads in one project at once: each turn's changes card counts only that thread's own files
- **Commits:** Threads live turn: one vocabulary for a running step's words; Threads turn counts: a shared checkout counts only the turn's own edits
- **Pins:** `apps/interface-host/src/threads_tests.rs::a_turn_in_a_shared_checkout_counts_only_the_files_its_own_edits_named`, `crates/interface-review/src/thread_git_tests.rs::scoped_turn_numstat_counts_only_named_files_inside_the_repository`

### Agent replies stream in steadily instead of arriving in chunks

- **Completed:** 2026-10-07T09:38:20-05:00
- **Kind:** look
- **Release:** v0.3.155
- **Commit:** `b6e0144` (`feat/threads-stream-20261007`)
- **Notes:** In a Threads pane an Opus reply flows in without pausing between bursts, and its last lines flow in instead of popping on when the turn ends
- **Pins:** `electron/src/renderer/conversation.test.ts::keeps an Opus-style burst flowing until the next one lands`, `electron/src/renderer/conversation.test.ts::finishes a completed message within 400 ms instead of snapping`

### New threads open at once in iCloud projects: checkpoints stop reading evicted git objects

- **Completed:** 2026-10-07T09:37:15-05:00
- **Kind:** fix
- **Release:** v0.3.155
- **Commit:** `60d12b1` (`fix/thread-launch-slow-20261007`)
- **Notes:** Start a thread in Kestrel AI (on the Desktop): it opens in under a second, not ~10 s, and its first turn has a checkpoint for Changes.
- **Commits:** Threads open at once in iCloud projects: checkpoints stop reading HEAD's trees
- **Pins:** `crates/interface-review/src/thread_git_tests.rs::checkpoint_capture_never_reads_head_trees_so_an_evicted_object_cannot_stall_it`

### Codex Untrusted mode: correct its description (the app server still accepts it)

- **Completed:** 2026-10-07T07:46:16-05:00
- **Kind:** fix
- **Release:** v0.3.154
- **Commit:** `7fb2f0f` (`fix/codex-untrusted-20261007`)
- **Notes:** The Codex Untrusted row no longer warns that sessions may refuse to start
- **Commits:** Codex "Untrusted" says what is true: the app server still takes it
- **Pins:** `electron/src/renderer/access-modes.test.ts::names Codex's, Grok's and the ACP providers' modes in their own words`

### Access modes use each provider's own names (Claude Manual, Codex Ask for approval, Grok)

- **Completed:** 2026-10-06T22:44:36-05:00
- **Kind:** look
- **Release:** v0.3.154
- **Commit:** `86dba02` (`feat/provider-mode-names-20261006`)
- **Notes:** The access picker and status line name each provider's modes as its CLI does, e.g. Claude Code's Manual / Accept edits / Plan / Auto
- **Commits:** Access modes use each provider's own names; Threads composer fixture: Claude Code's Plan mode is named Plan
- **Pins:** `electron/src/renderer/access-modes.test.ts::names Claude Code's modes as Claude Code does, picker and status line`, `electron/src/renderer/access-modes.test.ts::names Codex's, Grok's and the ACP providers' modes in their own words`

### Threads: record the color round and the inbox row pin it superseded

- **Completed:** 2026-10-06T22:18:43-05:00
- **Kind:** record
- **Release:** v0.3.153
- **Commit:** `b449c3e` (`feat/threads-ui-redesign-20261006`)
- **Notes:** TODO: the Threads color round is built; the inbox row pin moved with it
- **Supersedes:** 9fe5ab5 — the hued glow replaced the inbox row's provider pip with the project tile; its two-line row test is now "draws Pinned and Active as two lines: title and one slot, then tile, project, branch, the PR and the provider"

### Threads hued glow: project hues and tiles, status pills, hued composer, syntax-colored diffs

- **Completed:** 2026-10-06T21:57:19-05:00
- **Kind:** look
- **Release:** v0.3.153
- **Commit:** `abae39a` (`feat/threads-ui-redesign-20261006`)
- **Notes:** Threads: each project has a color; the open thread's bubbles, composer and Send take it; the inbox shows project tiles and status pills
- **Commits:** Hued glow: project hues, project tiles, file badges and the diffstat bar; Hued glow: a Threads pane in its project's hue; Hued glow: Threads inbox rows and the sidebar's header band; Hued glow: the Threads header band follows the focused thread's project; Hued glow: the New thread card and the Changes panel in colour; Settled looks: Threads hued glow, and the inbox's project tiles and pills
- **Pins:** `electron/src/renderer/project-hue.test.ts::is stable for a folder, whatever its trailing slash or case`, `electron/src/renderer/project-hue.test.ts::splits five squares by added and removed lines, showing each side that changed`

### Threads redesign: two-line inbox, folded turns, chip composer, centred new thread, diff comments

- **Completed:** 2026-10-06T21:04:13-05:00
- **Kind:** look
- **Release:** v0.3.153
- **Commit:** `8117db1` (`feat/threads-ui-redesign-20261006`)
- **Notes:** Threads: inbox rows show status or age, turns fold into Worked for…, Changes panel comments land in the draft
- **Commits:** Threads redesign: thread-layout flag and access-mode callback for Threads panes; Threads redesign: pane head as a breadcrumb with Changes totals and a pane menu; Threads: the New thread card becomes one centred column with a workspace drawer; Threads redesign: document the new pane head; Threads diff panel: scope menu, stacked files, hunks and line comments; Threads inbox: two-line rows, one slot with hover actions, shelves at the foot; Threads redesign: diff line comments land in the thread's draft; wider Changes column; Threads layout: folded work, changes card and controls once in a Threads pane's transcript; Threads redesign: the composer as one box with a + menu and run-setting chips; Threads redesign: one Stop per pane; Threads redesign: fixes from the hands-free pass; TODO: Threads review round 1 — the redesign, and what it left; Settled looks: Threads inbox rows become two lines with shelves
- **Pins:** `electron/src/renderer/components/ThreadInbox.test.ts::draws Pinned and Active as two lines: title and one slot, then pip, project, branch and the PR`, `electron/src/renderer/thread-transcript.test.ts::counts the folded work by kind: distinct files read, files edited, commands run`, `electron/src/renderer/components/ThreadDiffPanel.test.ts::follows the owner's words, one blank line apart, and never replaces them`, `electron/src/renderer/components/AgentTile.test.ts::reads as a breadcrumb: the project, then the title`

### TODO: Threads shipped in 0.3.152

- **Completed:** 2026-10-06T17:03:56-05:00
- **Kind:** record
- **Release:** v0.3.153
- **Commit:** `de9130d` (`record/threads-shipped-0.3.152`)
- **Notes:** TODO: Threads shipped in 0.3.152

### Threads: Chat becomes a T3-style inbox with project threads, worktrees, turn diffs and fork/edit

- **Completed:** 2026-10-06T13:38:13-05:00
- **Kind:** feature
- **Release:** v0.3.152
- **Commit:** `b745079` (`feat/threads-mode-20261006`)
- **Notes:** Threads tab: New thread picks a project (Local or New worktree); inbox sidebar; Cmd+D shows a turn's diff; Fork/Edit from here make new threads
- **Commits:** Threads mode: design, frozen contract and resume entry; Threads S0: wire skeleton for iface:threadControl and the iface:threads push; Threads: renderer hook for the Rust thread state; Threads S2: git mechanics for thread worktrees, checkpoints, diffs and restore; Threads S2: fork at a turn for Codex and Claude; Threads S3b: new thread card, thread pane head, turn controls and diff panel; Threads inbox: the Threads sidebar, its naming, menu, drags, Undo and chords; Threads: resume entry after S2/S3 merges; Threads S1: Rust thread state and the thread host; Hands-free: a scripted fake Claude for structured sessions; Threads I1: baseline refs, turn counting, steered sends, generated titles; Threads I2: settings, pull requests, card defaults and hands-free fixes; Settings test: check the Threads row tiles without a settled-look marker; Threads: resume entry after S1, I1 and I2; Threads F1: prompt titles for project threads, edits close the original, forks keep their mode; Threads F3: safe turn mapping, one fork per click, edit from turn 1 in steps, focus and visits; Threads F2: forks cut by prompt identity; worktrees, records and runs fail safe; Threads: ⌘Z undoes a settle or snooze made from the sidebar; Threads F4: kept worktrees say why, listings seed threads, card fixes; Threads: record the inbox look and the review handoff
- **Pins:** `crates/interface-review/src/thread_git_tests.rs::checkpoint_capture_never_changes_git_status_or_the_index`, `crates/interface-review/src/thread_git_tests.rs::restore_returns_to_a_checkpoint_and_keeps_a_recovery_ref`, `crates/interface-review/src/thread_git_tests.rs::ignored_files_keep_a_worktree_on_release_and_discard`, `apps/interface-host/src/threads_tests.rs::metadata_commands_answer_the_view_and_push_after_each_committed_change`, `apps/interface-host/src/threads_tests.rs::send_hook_records_a_turn_with_the_prompt_digest_and_its_start_checkpoint`, `electron/src/renderer/thread-inbox.test.ts::draws Rust's four sections from the shared v34 view, in inbox order`, `electron/src/renderer/thread-card.test.ts::offers the chat engines only with No project, every engine with a project`, `electron/src/renderer/thread-view.test.ts::puts the change summary and Edit from here on matched prompts only`

### To-do: performance review rounds 1 and 2 landed; what is left on purpose

- **Completed:** 2026-10-06T09:46:25-05:00
- **Kind:** record
- **Release:** v0.3.152
- **Commit:** `a52b928` (`docs/perf-review-todo-20261006`)
- **Notes:** To-do: performance review rounds 1 and 2 landed; what is left on purpose

### Claude panes turn their sidebar pip amber while a permission prompt or question waits

- **Completed:** 2026-10-06T09:33:40-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `7344a8d` (`feat/claude-waiting-signal-20261006`)
- **Notes:** A Claude pane asking for permission or asking you a question shows an amber pip and ⚠ in the sidebar until you answer
- **Commits:** Claude panes show "needs you" while a permission prompt or question waits
- **Pins:** `apps/interface-host/src/claude_terminal_hook_tests.rs::a_permission_dialog_marks_the_pane_waiting_without_deciding_it`, `electron/src/renderer/terminal-activity.test.ts::needs you from a permission or question frame until it is answered`

### To-do: Claude reviewer read-only repository tools done

- **Completed:** 2026-10-06T09:24:18-05:00
- **Kind:** record
- **Release:** v0.3.152
- **Commit:** `9dae1f0` (`record/claude-review-read-tools-20261006`)
- **Notes:** To-do: Claude reviewer read-only repository tools done; friction-fixes entry moved to done

### Version checks no longer time out on finished CLIs; retired settings channel; leaner host code

- **Completed:** 2026-10-06T09:22:58-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `d10e659` (`fixall/host-20261006`)
- **Notes:** A Claude, Codex or OpenCode version check returns when the CLI exits, even if another process holds its output pipe
- **Commits:** Retired terminal settings channel answers "unavailable"; one less lock per keystroke; Editor folder listings name each entry with one lstat; Agent teammate steps skip two whole-window passes per mutation; Provider version checks and bounded commands finish when the command does; One shared way to run ps for pane identity probes; Provider session lookups share one capture-owner check; Rename the provider capture lookup after what it serves; One directory-pinning check for the workspace editor and MCP socket
- **Pins:** `crates/interface-engine/src/executable.rs::a_version_probe_ends_when_the_cli_exits_though_its_pipe_stays_open`, `crates/interface-engine/src/child_output.rs::output_ends_with_the_child_though_an_outside_process_holds_the_pipe`

### Shorter root AGENTS.md: hands-free and release traps move to their own docs

- **Completed:** 2026-10-06T09:19:45-05:00
- **Kind:** record
- **Release:** v0.3.152
- **Commit:** `2258e44` (`docs/agents-md-trim-20261006`)
- **Notes:** Shorter root AGENTS.md: hands-free and release traps move to their own docs

### Hidden terminals skip screen-reader work; two DOM-lane flakes fixed; App.tsx slimmer

- **Completed:** 2026-10-06T09:18:45-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `4fb7973` (`fixall/renderer-20261006`)
- **Notes:** Renderer DOM lanes run 4 at a time beside the Rust tests; terminals out of view for a minute stop building announcements
- **Commits:** Hidden terminals stop building screen-reader announcements; Workspace hierarchy check waits for the sidebar slide, not 400 ms; Theme check waits for the focus ring's transition before measuring; Gate runs four renderer DOM lanes beside the Rust tests; App.tsx sheds the session tile and the palette's command list; Worker cancel test bounds a durable cancel, not a race

### iface theme convert: turn iTerm2, Ghostty, Alacritty, Base16 or VS Code schemes into theme files

- **Completed:** 2026-10-06T08:55:25-05:00
- **Kind:** feature
- **Release:** v0.3.152
- **Commit:** `b999221` (`feat/theme-convert-20261006`)
- **Notes:** Run iface theme convert <scheme file> > ~/.config/interface/themes/Name.css and pick the theme in Settings
- **Commits:** Theme import: iface theme convert turns other apps' colour schemes into theme files; To-do: theme convert landed on Dev; reading scheme files directly stays open
- **Pins:** `crates/interface-settings/src/theme_import_tests.rs::converted_css_reads_back_as_a_theme_file`, `apps/iface/src/theme.rs::theme_convert_prints_css_and_suggests_the_file_name`

### Voice assistant sees the 50 most recent sessions, not all of them

- **Completed:** 2026-10-06T08:48:04-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `f302f03` (`fix/voice-session-cap-20261006`)
- **Notes:** Ask the voice assistant about your sessions: it answers from the 50 most recent and can still find an older one by name
- **Pins:** `apps/interface-host/src/voice/tools.rs::the_assistant_sees_the_fifty_most_recent_sessions_and_the_full_count`, `apps/interface-host/src/voice/tools.rs::a_query_finds_older_sessions_by_title_folder_or_engine`

### Shell handoff stays in its own workspace after a switch

- **Completed:** 2026-10-06T08:45:33-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `eaa08aa` (`fix/handoff-own-workspace-20261006`)
- **Notes:** Switch workspace while an exited agent's shell is starting: the shell opens in the agent's own workspace, not the one you switched to
- **Pins:** `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalHandoffSwitchFixture`

### Gate stops rerunning two load-flaky tests and keeps receipts across an Xcode or node change

- **Completed:** 2026-10-06T08:41:28-05:00
- **Kind:** internal
- **Release:** v0.3.152
- **Commit:** `d901be5` (`fixall/iface-20261006`)
- **Notes:** The gate no longer reruns two tests that only failed under load; Gate receipts survive an Xcode or node change on the other side; Control-desk child test keeps reading the tool-less host until it exits

### Model-facing prompts and tool descriptions match what the tools do

- **Completed:** 2026-10-06T08:35:15-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `737dd2b` (`prompt-audit-20261006`)
- **Notes:** Claude panes show the cheap-agents instructions once, and the voice assistant no longer treats every session as text rehearsal
- **Pins:** `apps/interface-host/src/pane_mcp.rs::pane_tools_name_only_ids_pane_sessions_can_obtain`, `crates/interface-collaboration/src/workers/tools.rs::worker_tools_describe_the_contracts_their_handlers_enforce`, `crates/engine-claude/src/adapter_tests.rs::voice_prompt_does_not_key_speech_on_the_constant_audio_flag`

### Quieter host while idle and streaming; new panes no longer fail when a job grabs the terminal first

- **Completed:** 2026-10-06T08:32:03-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `99273fe` (`fixall/host-20261006`)
- **Notes:** A pane whose shell starts a job at once opens instead of failing; the sidebar re-sorts by activity at most every 5 s
- **Pins:** `crates/interface-terminal/src/embedded.rs::spawn_identity_accepts_a_job_that_already_took_the_foreground`, `crates/interface-terminal/src/foreground_profile.rs::a_job_without_an_agent_backs_off_to_thirty_seconds_after_its_first_minute`, `crates/interface-remote/src/access.rs::output_is_kept_only_while_phone_access_is_on_and_always_counted`

### Review panes stay open in Chat; Open in external terminal works again

- **Completed:** 2026-10-06T08:28:59-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `ed1dabb` (`fixall/renderer-20261006`)
- **Notes:** Builder/reviewer panes open in Chat and stay across refreshes; Open in external terminal is enabled on Code's terminals
- **Commits:** Gate: the collaboration state is read by a renderer test
- **Pins:** `electron/src/renderer/conversation-layouts.test.ts::keeps an adversarial-review pair open across registry pushes`, `electron/src/renderer/conversation-projection.test.ts::keeps a sent message's echo under the row it followed when compaction drops the head`, `electron/src/renderer/draft-store.test.ts::publishes a typed word once, its clearing once, and every send's republish`

### New panes split the pane they open from

- **Completed:** 2026-10-06T08:17:26-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `53d02e3` (`fix/split-pane-in-place`)
- **Notes:** A pane's + → Terminal halves that pane (side by side if wide, stacked if tall); no other pane moves and the new pane fades in once
- **Pins:** `electron/src/renderer/terminal-workspace.test.ts::halves only the pane it was opened from; no other pane moves`, `electron/src/renderer/terminal-workspace.test.ts::gives the same layout when React runs the updater again`, `electron/src/renderer/fixtures/terminal-pane-motion-fixture.tsx::terminalPaneMotionFixture`

### iface agent list fits any registry; agent stop never waits on another agent's start

- **Completed:** 2026-10-06T08:12:35-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `d69cfcf` (`fixall/iface-20261006`)
- **Notes:** iface agent list answers with thousands of stored sessions (live first, --live for only live); iface agent stop returns while another agent is still starting
- **Commits:** iface agent list fits any registry, stop answers at once, mailbox no longer fills up
- **Pins:** `apps/iface/src/agent_server.rs::a_long_list_keeps_live_and_named_sessions_and_fits_one_reply`, `apps/iface/src/agent_server.rs::a_stop_does_not_wait_for_another_sessions_start_or_resume`, `apps/iface/src/agent_server.rs::the_mailbox_prunes_settled_messages_but_keeps_unknown_ones_and_their_parents`, `apps/iface/src/tui_tests.rs::a_huge_diff_keeps_a_capped_side_and_details_say_so`

### Codex adapter and session management split into smaller files; review to-do entry

- **Completed:** 2026-10-06T01:27:12-05:00
- **Kind:** internal
- **Release:** v0.3.152
- **Commit:** `fc74063` (`perf/split-20261005`)
- **Notes:** Codex adapter and session management code split into smaller files; To-do: October 5 performance review — what landed and what waits on the owner; Gate reruns two load-flaky tests once instead of failing

### Faster gate: Rust tests run four at a time beside Clippy; steadier timing tests

- **Completed:** 2026-10-06T01:02:25-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `3564679` (`perf/gate-20261005`)
- **Notes:** cargo xtask check prints a per-executable time table; the Rust half takes about 55 s instead of about 105 s; an Electron-only gate runs 5 renderer lanes
- **Commits:** Faster gate: Rust test executables run four at a time; Steadier tests under load: three timing races closed; Name the gate test runner after what it is

### Disabled pane delegation no longer built by default; simpler orchestration and capture-hook code

- **Completed:** 2026-10-06T00:22:00-05:00
- **Kind:** internal
- **Release:** v0.3.152
- **Commit:** `fc91a9f` (`perf/rust-20261005`)
- **Notes:** Disabled pane delegation no longer built by default; simpler orchestration and capture-hook code

### One saved-layout parse for terminals; App.tsx and conversation modules untangled

- **Completed:** 2026-10-06T00:08:31-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `dc64182` (`perf/renderer-20261005`)
- **Notes:** Splitter drags and roster reads no longer re-parse every saved workspace layout
- **Commits:** One saved-layout parse for every terminal reader, and no conversation re-export barrel; App.tsx sheds its engine table and subagent lookups into tested modules

### Less background work: no usage reads while hidden, lighter streaming, one save per slider drag

- **Completed:** 2026-10-05T23:56:53-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `a2c3697` (`perf/renderer-20261005`)
- **Notes:** With the window hidden no usage CLI is spawned; dragging a translucency slider writes appearance once; long chats stream with less lag

### Fewer drive flushes and process spawns; faster Quick Open and Claude listing at launch

- **Completed:** 2026-10-05T23:54:33-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `36c392f` (`perf/rust-20261005`)
- **Notes:** Quick Open opens faster on large projects; each store write and decision-log append waits on at most one full drive flush

### Every agent's Dev refresh stays hidden, even from older branches

- **Completed:** 2026-10-05T23:39:23-05:00
- **Kind:** internal
- **Release:** v0.3.152
- **Commit:** `dacee77` (`fix/quiet-dev-refresh-20261005`)
- **Notes:** Every agent's Dev refresh stays hidden, even from older branches

### Terminals give back GPU memory when out of view, and the work area drops a hidden layer

- **Completed:** 2026-10-05T23:34:56-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `50885bd` (`perf/renderer-20261005`)
- **Notes:** Leave a workspace or tab for a minute, or hide the window for two: the GPU helper's footprint drops; panes come back on WebGL with crisp text

### Lighter registry pushes; pane output no longer stalls behind sleep probes or agent saves

- **Completed:** 2026-10-05T23:31:24-05:00
- **Kind:** speed
- **Release:** v0.3.152
- **Commit:** `19703e7` (`perf/rust-20261005`)
- **Notes:** Switching away from a workspace no longer pauses pane output; host CPU stays lower while agents stream
- **Commits:** Lighter registry pushes and terminal sleep that never stalls pane output; Phone roster keeps its pinned grouping test unchanged

### Dev refresh no longer takes the screen

- **Completed:** 2026-10-05T23:28:01-05:00
- **Kind:** internal
- **Release:** v0.3.152
- **Commit:** `e34b28b` (`fix/quiet-dev-refresh-20261005`)
- **Notes:** Dev refresh no longer takes the screen

### Codex chats keep working after a failed turn; stale socket and temp files get cleaned up

- **Completed:** 2026-10-05T22:53:18-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `cacc353` (`perf/rust-20261005`)
- **Notes:** After a failed Codex turn (e.g. a usage limit) the composer still sends; after a Dev restart the collaboration sockets folder holds only live sockets
- **Commits:** Codex chats keep working after a failed turn, and stale files stop piling up
- **Pins:** `crates/interface-engine/src/lifecycle.rs::notice_does_not_move_status_and_a_failed_turn_keeps_the_handle_sendable`, `crates/engine-codex/src/adapter_tests.rs::a_failed_turn_leaves_the_session_sendable`, `crates/interface-storage/src/atomic.rs::a_write_that_fails_before_the_rename_removes_its_temporary`, `crates/interface-collaboration/tests/suite/broker_runtime.rs::start_removes_dead_sockets_and_leaves_live_ones_and_other_files`

### Chat rows stay put through compaction, snake_case renders intact, quit never strands the host

- **Completed:** 2026-10-05T22:50:45-05:00
- **Kind:** fix
- **Release:** v0.3.152
- **Commit:** `56d4380` (`perf/renderer-20261005`)
- **Notes:** max_tokens keeps its underscores in chat; the Dev log stops filling with stale collaboration-draft revisions; quitting after a window crash still stops the host
- **Pins:** `electron/src/renderer/draft-store.test.ts::stays above the host's mark after a reload forgets the counter`, `electron/src/renderer/markdown.test.ts::leaves intraword underscores and spaced asterisks as the characters they are`, `electron/src/main/main-wiring.test.ts::stops the host even when an earlier teardown step throws`, `electron/src/renderer/conversation-projection.test.ts::keeps every surviving row's key and item when compaction drops the log's head`

### Phone relay on Cloudflare Workers (off-Wi-Fi phone access)

- **Completed:** 2026-10-05T22:44:02-05:00
- **Kind:** internal
- **Release:** v0.3.152
- **Commit:** `840c7b0` (`feat/phone-relay-worker-20261005`)
- **Notes:** Phone relay on Cloudflare Workers; TODO: phone relay setup steps for the owner

### Keep the main checkout on dev; looser release and look approvals

- **Completed:** 2026-10-05T22:39:05-05:00
- **Kind:** internal
- **Release:** v0.3.152
- **Commit:** `a4c17eb` (`docs/agent-friction-approved-20261005`)
- **Notes:** Keep the main checkout on dev; looser release and look approvals

### Narrower agent overview column (360–560px)

- **Completed:** 2026-10-05T22:20:47-05:00
- **Kind:** look
- **Release:** v0.3.152
- **Commit:** `972a887` (`fix/narrow-right-sidebar-20261005`)
- **Notes:** The right-hand agent overview is narrower: 560px on a 2560pt window, 360px at 1440px
- **Commits:** Narrower agent overview column
- **Pins:** `electron/src/renderer/agent-overview.test.ts::takes 360px at a 1440px window and stops at 560px on a large display`

### To-do: agent workflow friction fixes awaiting the owner

- **Completed:** 2026-10-05T22:17:52-05:00
- **Kind:** record
- **Release:** v0.3.152
- **Commit:** `1bd070d` (`docs/agent-friction-20261005`)
- **Notes:** To-do: agent workflow friction fixes awaiting the owner

### Lighter agent start-up rules and a shorter to-do list

- **Completed:** 2026-10-05T22:13:21-05:00
- **Kind:** record
- **Release:** v0.3.152
- **Commit:** `dd51cc0` (`docs/agent-friction-20261005`)
- **Notes:** Lighter agent start-up rules and a shorter to-do list

### New projects land in ~/Developer/Projects

- **Completed:** 2026-10-03T21:49:00-05:00
- **Kind:** fix
- **Release:** v0.3.151
- **Commit:** `457a17f` (`fix/new-project-developer-20261003`)
- **Notes:** Workspaces + → New project… shows ~/Developer/Projects/<name> and creates the folder there
- **Pins:** `crates/interface-desktop-services/src/service_tests.rs::create_project_creates_and_grants_the_exact_named_folder_from_the_shared_fixture`

### Theme files: your own .css themes in ~/.config/interface/themes

- **Completed:** 2026-10-02T12:00:57-05:00
- **Kind:** feature
- **Release:** v0.3.151
- **Commit:** `5afb4a4` (`feat/custom-themes-20261002`)
- **Notes:** A .css file in ~/.config/interface/themes shows under Yours in Settings; picking it repaints the app, and saving the file repaints it live
- **Commits:** docs: custom themes handoff (design stage); Theme files keep neutral tile edges and a lifted focused head; docs: custom themes closeout waits on three settled-look approvals; Theme files keep three settled pane looks
- **Pins:** `crates/interface-desktop-services/src/service_tests.rs::theme_files_v5_list_and_paint`, `electron/src/renderer/components/SettingsPage.test.ts::lists theme files under Yours and pairs Follow macOS with any theme`, `electron/src/renderer/theme-files.test.ts::sit inside the interface-palette layer, so a file's plain :root rule wins`, `apps/interface-host/src/theme_watch.rs::creates_the_folder_and_pushes_once_per_change`

### Roomier look: 32px heads and rows, 12px gutters, airier terminal lines

- **Completed:** 2026-10-02T11:03:10-05:00
- **Kind:** look
- **Release:** v0.3.150
- **Commit:** `b211604` (`feat/roomier-d-20261002`)
- **Notes:** Pane heads and sidebar rows are 32px, gutters 12px, terminal lines visibly airier, and the agent overview cards are larger
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps adjacent block-art and box-drawing rows visually joined`, `electron/src/renderer/theme-accessibility.test.ts::pins visible focus, minimum shared hitboxes, and container-aware pane layout`
- **Supersedes:** a3a8d55 — the owner chose roomier proportions (D, 2026-10-02); the 212px sidebar and 8px gutters give way to 240/264px and 12px

### Sidebar pips take their agent's colour: Codex green, Claude rust

- **Completed:** 2026-10-02T10:43:49-05:00
- **Kind:** fix
- **Release:** v0.3.150
- **Commit:** `781ecf3` (`fix/provider-pips-20261002`)
- **Notes:** A workspace with a Claude and a Codex pane shows a rust pip and a green pip; a working shell's pip is grey
- **Pins:** `electron/src/renderer/viewmodels.test.ts::names each agent pane's provider so Codex and Claude pips differ`, `electron/src/renderer/components/Sidebar.test.ts::keeps its visual rules component-scoped and token driven`
- **Supersedes:** f76e6cf — the assertion is reworded: one pip per open pane, urgent first, is unchanged, but each pip is now an object naming its light and provider; b0fae24 — pips painted by status alone are replaced: working and idle agent pips take their provider's colour

### Polished workspace launcher: setup state, joined count, folder name, ⌘↵ launch

- **Completed:** 2026-10-01T21:22:16-05:00
- **Kind:** look
- **Release:** v0.3.149
- **Commit:** `8c511c2` (`feat/launcher-refine-20261001`)
- **Notes:** New workspace shows no intro line, one joined 1–8 control, the folder's name before ~/path, and ⌘↵ launches
- **Commits:** Polished workspace launcher (Refine 1)
- **Pins:** `electron/src/renderer/components/TerminalLauncher.test.ts::shows a home folder the way a terminal prints it, keeping the absolute path`, `electron/src/renderer/components/TerminalLauncher.test.ts::counts the shared folder note and launches on ⌘↵ only`

### Colourful update button: a blue-violet tile with its words

- **Completed:** 2026-10-01T20:20:40-05:00
- **Kind:** look
- **Release:** v0.3.148
- **Commit:** `1e08ada` (`feat/update-button-d4-20261001`)
- **Notes:** When an update exists, the rail shows a blue-violet tile and gradient words; ready adds a popping dot and drifting colour
- **Pins:** `electron/src/renderer/components/UpdateButton.test.ts::marks ready so the dot pops and the words drift`, `electron/src/renderer/components/UpdateButton.test.ts::turns the tile into a ring that fills with the download`

### Sidebar pips: one per open pane, urgent ones first

- **Completed:** 2026-10-01T17:27:57-05:00
- **Kind:** fix
- **Release:** v0.3.147
- **Commit:** `c492329` (`fix/sidebar-pips-20261001`)
- **Notes:** A workspace with 3 open panes shows 3 pips; a finished or waiting pane's pip shows even past 8
- **Pins:** `electron/src/renderer/viewmodels.test.ts::drops exited terminals and caps the row with a count`, `electron/src/renderer/components/Sidebar.test.ts::counts open tabs without dropping saved members from workspace identity`

### A finished agent's pane gets its shell back even if its workspace was parked (Codex update)

- **Completed:** 2026-10-01T17:21:19-05:00
- **Kind:** fix
- **Release:** v0.3.147
- **Commit:** `d99a87f` (`fix/codex-update-restart`)
- **Notes:** Update Codex in a pane and switch workspace meanwhile: coming back, the pane is a working shell, not a dead 'Update ran successfully' screen.
- **Commits:** Hand a finished agent's pane to a shell even when its workspace was parked
- **Pins:** `electron/src/renderer/fixtures/terminal-exit-handoff-fixture.tsx::terminalExitHandoffFixture`

### TODO: hidden production terminals left for the owner

- **Completed:** 2026-10-01T12:52:33-05:00
- **Kind:** record
- **Release:** v0.3.147
- **Commit:** `4f6188f` (`fix/orphan-terminals-20261001`)
- **Notes:** TODO: hidden production terminals left for the owner

### Closing a tab ends its terminal; hidden terminals are cleaned up automatically

- **Completed:** 2026-10-01T12:50:42-05:00
- **Kind:** fix
- **Release:** v0.3.147
- **Commit:** `d8dc83f` (`fix/orphan-terminals-20261001`)
- **Notes:** Closing a tab or pane ends its process, and a relaunch no longer respawns closed tabs unseen
- **Commits:** Closing a tab ends its terminal; nothing runs where no pane shows it; Name the pane close test for what closing now does
- **Pins:** `electron/src/renderer/terminal-workspaces.test.ts::does not respawn an agent whose tab an earlier run closed`, `electron/src/renderer/terminal-hidden-sweep.test.ts::ends a quiet hidden terminal only once the whole grace has passed`
- **Supersedes:** 3726446 — closing a tab now ends its terminal instead of detaching it

### TODO: 0.3.146 shipped the dense look and the September 30 fixes

- **Completed:** 2026-10-01T12:10:10-05:00
- **Kind:** record
- **Release:** v0.3.147
- **Commit:** `efabd9d` (`docs/shipped-03146-20261001`)
- **Notes:** TODO: 0.3.146 shipped the dense look and the September 30 fixes

### TODO: the owner declined a context figure in pane heads

- **Completed:** 2026-10-01T12:02:40-05:00
- **Kind:** record
- **Release:** v0.3.146
- **Commit:** `dd20a13` (`docs/no-context-20261001`)
- **Notes:** TODO: the owner declined a context figure in pane heads

### Closer to the reference: overview tabs and toggle, card pills, 8px gutters

- **Completed:** 2026-10-01T11:20:12-05:00
- **Kind:** look
- **Release:** v0.3.146
- **Commit:** `45bd9d9` (`feat/dense-closer-20261001`)
- **Notes:** Dark theme: Agents/Tasks tabs and a status pill on each overview card; overview toggle at the rail's right edge; 8px gaps
- **Pins:** `electron/src/renderer/agent-overview.test.ts::shows a live card's last lines, its clock, and how to open a waiting pane`, `electron/src/renderer/workspace-ui.test.ts::uses integrated macOS chrome with a centered surface switch and Code destinations`

### Dense look: flush tiles, live agent cards and sidebar pips

- **Completed:** 2026-10-01T11:03:00-05:00
- **Kind:** look
- **Release:** v0.3.146
- **Commit:** `117dc3f` (`feat/dense-look-20261001`)
- **Notes:** Dark theme: flush black panes with thin heads, one status pip per pane in the sidebar, and live cards showing each agent's last lines in the overview
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::draws a rounded selection box around the focused terminal pane`, `electron/src/renderer/agent-overview.test.ts::shows a live card's last lines, its clock, and how to open a waiting pane`, `electron/src/renderer/viewmodels.test.ts::draws one light per terminal, in order, with the pane's own precedence`

### September 30 review handoff: release ready for the owner

- **Completed:** 2026-10-01T08:10:30-05:00
- **Kind:** record
- **Release:** v0.3.146
- **Commit:** `9726190` (`docs/review-handoff-20260930`)
- **Notes:** TODO: the September 30 review's release is ready for the owner to promote

### Quieter terminal ticker, phone roster reads and TUI draw loop; MCP sockets re-verified

- **Completed:** 2026-10-01T08:08:33-05:00
- **Kind:** speed
- **Release:** v0.3.146
- **Commit:** `3c83149` (`perf/wave2-20260930`)
- **Notes:** Idle panes no longer start helper threads every second, and phone workspace lists stay responsive during saves
- **Commits:** Phone roster reads, the ticker and the TUI draw loop do less; MCP sockets re-verified

### Less work per token, pane, output chunk and window focus; quicker quits and gate

- **Completed:** 2026-10-01T08:02:54-05:00
- **Kind:** speed
- **Release:** v0.3.146
- **Commit:** `46d3580` (`perf/fixes-20260930`)
- **Notes:** Busy agent panes stay smooth, quitting with many panes is quicker, and a window focus no longer re-renders the app
- **Commits:** Registry updates after sends and approvals join the 50 ms coalescer; Away and control-desk loops do less idle work; Claude events move through a single-consumer queue; Collaboration samples registry rows only for resolved sources; Hidden panes whose job keeps printing stop re-probing every second; Disposable worker packets skip drive flushes and the unread baseline copy; Teardown retries back off instead of polling every 50 ms; Workspace sleep reads the process table twice per attempt; Hook helpers stop forking for liveness and the notice round trip; Pane tools look a pane up directly; Quitting closes panes side by side; the bridge drops parsed request text; Agent, harness, phone and updater paths stop copying what they only read; Claude listings and tool summaries copy less; Grok history and worker reports stop copying text they only inspect; The renderer does less per output chunk, focus and resize; Terminal output acks leave in one write per event-loop turn; The gate skips the empty doc-test pass and runs Swift beside Rust; iface redraws less on resize and for non-ASCII text; Delegation reads again only when a hidden pane is drawn again

### Long Claude answers end their turn; registries keep saving; menus stay open while chats stream

- **Completed:** 2026-10-01T07:37:38-05:00
- **Kind:** fix
- **Release:** v0.3.146
- **Commit:** `b1bc3d9` (`fix/edge-cases-20260930`)
- **Notes:** A Claude pane gets its done dot after a long answer, and the model picker stays open while a reply streams
- **Commits:** Claude panes finish turns whose answer is long; Large session registries and orchestration stores keep saving; Harness, pane sockets and Codex sessions recover from edge cases; Claude replies read in the same chunk as a bad line still settle; read_pane keeps an exited pane's output while the pane is shown; Menus, the hover card and terminal input survive unrelated events; iface: details overlay scrolls back at once; resumed sessions keep their commands; Codex steering lives in its own function; Backtick SessionStart in the hook bound's doc
- **Pins:** `apps/interface-host/src/claude_terminal_hook_tests.rs::long_turn_boundary_payloads_still_report_activity`, `crates/interface-registry/src/session_tests.rs::a_registry_of_ten_thousand_sessions_still_saves_and_reloads`, `crates/engine-codex/src/adapter_tests.rs::a_refused_first_turn_keeps_the_owed_collaboration_source`, `electron/src/main/terminal-data-transport.test.ts::keeps a late input frame a no-op after host EOF and the pane's release`, `electron/src/renderer/terminal-activity.test.ts::ignores OSC 9 from panes that are not Codex`, `electron/src/renderer/components/scroll-anchor.test.ts::counts only scrolls of the anchor's ancestors and the viewport`

### Recover stopped orphan Claude panes

- **Completed:** 2026-09-30T00:25:57-05:00
- **Kind:** fix
- **Release:** v0.3.146
- **Commit:** `d702526` (`fix/claude-orphan-recovery-20260929`)
- **Notes:** Saved Claude panes reopen their original conversations after stopped processes outlive a host.
- **Commits:** Recover stopped orphan Claude terminal captures
- **Pins:** `apps/interface-host/src/claude_terminal_session_tests.rs::startup_recovers_real_stopped_orphans_and_preserves_the_session_across_two_cold_starts`, `apps/interface-host/src/shutdown.rs::terminal_cleanup_finishes_before_a_slow_service_drain_starts`, `crates/interface-terminal/src/embedded/orphan.rs::orphan_recovery_requires_exact_stopped_owned_group_and_absent_parent`

### Ten themes, their own terminal colours and the UI review polish

- **Completed:** 2026-09-28T22:17:31-05:00
- **Kind:** look
- **Release:** v0.3.144
- **Commit:** `e1ea1a9` (`feat/ui-review-20260928`)
- **Notes:** Settings shows ten theme swatches; light-theme terminals print readable colours; a pane waiting on you shows an amber strip
- **Pins:** `electron/src/renderer/terminal-palettes.test.ts::keeps every colour legible on its own screen`, `electron/src/renderer/components/SettingsPage.test.ts::picks themes from swatches and switches, never a native select`, `electron/src/shared/appearance.test.ts::maps the retired palettes to the themes that replaced them`
- **Supersedes:** 058e010 — the New pane menu's last rows are sentence case (Split down, New tab, New session…), owner 2026-09-28; c353b8f — the ··· menu's layout rows are sentence case (Tidy panes, Flip terminals, Saved sessions…), owner 2026-09-28; 784e597 — terminal text clears the rounded screen by 10px instead of the full-width entry bar, owner 2026-09-28

### Codex-update dead pane investigation notes

- **Completed:** 2026-09-28T20:10:05-05:00
- **Kind:** record
- **Release:** v0.3.143
- **Commit:** `96a6a58` (`fix/codex-update-pane`)
- **Notes:** Record the Codex-update dead pane investigation

### Browser pane opens Google and searches from the address bar

- **Completed:** 2026-09-28T19:54:32-05:00
- **Kind:** feature
- **Release:** v0.3.143
- **Commit:** `d6c7b44` (`feat/browser-home-search-20260928`)
- **Notes:** Pane + → Browser opens Google with the address bar focused; typing words searches Google; a running dev server shows as a localhost button
- **Pins:** `electron/src/renderer/browser-pane.test.ts::searches Google for anything that is not an address`, `electron/src/renderer/browser-pane.test.ts::opens a new tab on Google`

### Collapse all folders from the sidebar header

- **Completed:** 2026-09-28T18:24:56-05:00
- **Kind:** feature
- **Release:** v0.3.143
- **Commit:** `e9353c4` (`feat/collapse-all-folders-20260928`)
- **Notes:** A button after Filter in the Workspaces header folds every folder; clicked again it opens them all
- **Pins:** `electron/src/renderer/components/Sidebar.test.ts::offers Collapse all while any project is open and Expand all once every one is shut`, `electron/src/renderer/components/Sidebar.test.ts::collapses the listed projects and reopens only those`

### Command+C copies Codex's own selection

- **Completed:** 2026-09-28T17:31:20-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `0dcc59a` (`fix/codex-pane-copy-20260928`)
- **Notes:** In a Codex pane, drag over text, press Command+C: Codex says Copied N chars and the text pastes in other apps
- **Pins:** `electron/src/renderer/terminal-keys.test.ts::sends ⌘C to a program that owns the pointer and reads kitty key reports`, `electron/src/renderer/terminal-replay-buffer.test.ts::restores Codex's kitty keyboard flags on the screen that pushed them`

### Browser is a pane in the split, like every other pane

- **Completed:** 2026-09-28T17:12:51-05:00
- **Kind:** feature
- **Release:** v0.3.142
- **Commit:** `17ca80d` (`feat/browser-pane-leaf-20260928`)
- **Notes:** Pane + > Browser opens a Browser pane beside it; its page survives splits, tidy, maximize and workspace switches, and reopens after restart
- **Commits:** Browser page layer styles live with the terminal centre; Keep the terminal close label the packaged smoke pins; Browser panes follow the settled auto-tidy rule
- **Pins:** `electron/src/renderer/terminal-workspace.test.ts::is a pane of its own beside the source, and no terminal`, `electron/src/renderer/terminal-workspace.test.ts::keeps its page through a save, and survives the host roster reconciling`

### Record the finished cleanup and the decisions left for the owner

- **Completed:** 2026-09-27T13:00:20-05:00
- **Kind:** record
- **Release:** v0.3.142
- **Commit:** `c761aa2` (`record/review-20260927-final`)
- **Notes:** Record the finished cleanup and the decisions left for the owner

### Phone relay reconnects back off; iPhone lock revokes over its proven route

- **Completed:** 2026-09-27T12:59:42-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `b6dd78f` (`land/review-20260927-r12`)
- **Notes:** A relay that stays down is retried at 3, 6, 12, 24 then 30 s; a session that reached it retries in 3 s
- **Commits:** Relay reconnects back off; one pairing QR, compare and header path; Host remote dispatch decodes each action once and is tested; iPhone lock revokes over the proven route; one Mac identity check; iPhone lock revoke falls back to discovery when its route never took it
- **Pins:** `crates/interface-remote/src/http/direct_tests.rs::relay_retries_back_off_to_thirty_seconds_and_restart_after_a_session`

### Host collaboration and orchestration split; packet and transcript rules move into their crates

- **Completed:** 2026-09-27T12:54:34-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `2672322` (`land/review-20260927-r26`)
- **Notes:** The collaboration packet builder moves into its crate; the host module splits; Orchestration's transcript rules move into their crate; the host module splits; One registry access module and one push module serve every host owner; Owners share one payload module and dispatch on CHANNELS; Setting changes, synthetic events and kept cleanups each build once; A failed host startup closes what it opened through one teardown; Provider resolvers, path guards and Codex overrides are written once

### xtask: one checked-file open, gate runner module, worktree parser, typed promotion phases

- **Completed:** 2026-09-27T12:36:26-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `b7e0652` (`land/review-20260927-r28`)
- **Notes:** One checked-file open for release and packaging; digests format themselves; Gate execution leaves the packaging module; both apps pass one check list; Every xtask git child gets a transfer deadline; one worktree parser; Promotion phases are typed; one promotion lock, report and git fixture

### Host terminal dispatch split into arms; one module owns capture generations

- **Completed:** 2026-09-27T12:27:30-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `9d3a4e2` (`land/review-20260927-r25`)
- **Notes:** Host terminal helpers are written once; Pin the terminal dispatch's authority refusals; Terminal dispatch arms move into their own modules; Terminal authority refusals and desktop errors are built once; One module owns the capture-generation layout; Claude session lookup splits into protocol, receipts, registry and probes

### Terminal signals and liveness checks without /bin/kill forks

- **Completed:** 2026-09-27T12:24:04-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `f9ecde2` (`land/review-20260927-ct8`)
- **Notes:** Closing a shell pane spawns one process instead of four
- **Commits:** Terminal signals and liveness checks no longer fork /bin/kill

### Checkpoints flush the drive twice instead of per file; release binaries report the product version

- **Completed:** 2026-09-27T12:11:38-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `8a22c47` (`land/review-20260927-r35`)
- **Notes:** A 40-file turn checkpoint takes ~15 ms instead of ~200 ms; iface --version prints the release version
- **Commits:** Turn checkpoints flush the drive cache twice instead of once per file; Release binaries report the product version; A new release version recompiles only the two binaries' own targets; Package parity pin follows the reused cargo target
- **Pins:** `apps/iface/src/app_tests.rs::version_prints_the_product_version_without_touching_state`

### Terminal create and restart share one spawn path; service.rs split; bindings follow one rule

- **Completed:** 2026-09-27T11:35:11-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `505a8f6` (`land/review-20260927-r24`)
- **Notes:** A restart and a create of the same pane produce identical entries; a cwd-changed spawn keeps its lease until teardown
- **Commits:** Terminal tests share one gate and one wait helper; Pin that create and restart install and undo a generation alike; A spawn whose cwd changed keeps its lease until teardown is proven; Create and restart spawn and install a generation through one pair; Provider session bindings follow one rule everywhere; Wait for the fixture servers' ports, not just their port files; Terminal crate helpers are written once; Terminal service types, output, pass-off and caller lookup move out of service.rs; Terminal process tasks and teardown move out of service.rs
- **Pins:** `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_failed_restart_undoes_each_spawn_step`

### Two more load-flaky tests fixed: Codex start-error race, Qwen trap order

- **Completed:** 2026-09-27T11:32:33-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `46d1381` (`land/review-20260927-flaky2`)
- **Notes:** Two more load flakes: Codex start-error wording race, Qwen trap order

### Harness and control desk stop refusing every write when full; Stop always records

- **Completed:** 2026-09-27T11:15:15-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `050212f` (`land/review-20260927-r32`)
- **Notes:** A full control history still records Stop and deadline expiry; harness drops its oldest resolved items
- **Commits:** Harness history drops its oldest resolved items instead of filling up; Control desk keeps room for Stop and compacts old settled runs
- **Pins:** `crates/interface-collaboration/src/voice_control.rs::a_full_history_still_records_stop_and_interruptions_in_the_reserved_room`

### iface TUI split into reducer, approval, collaboration, orchestration and terminal modules

- **Completed:** 2026-09-27T10:50:52-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `e8b3c74` (`land/review-20260927-r27`)
- **Notes:** Split the iface TUI into reducer, approval, collaboration, orchestration and terminal modules; One orchestration agent listing, a NewSession for starts, and a composer key table; Shared iface CLI parsers and smaller command helpers

### Agent policy and desktop service split into modules; one terminal-state lock

- **Completed:** 2026-09-27T10:44:53-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `334950b` (`land/review-20260927-r23`)
- **Notes:** Split the agent approval policy into modules; Agent policy tables share one list per family; Saved terminal roster and restart authority share one lock; Desktop service state machines move into their own modules; Workspace operations share one authority prologue; ACP profile saves apply one set of field rules; Keep agent_policy.rs as the module root with its tests inline

### iface back to 0.1.169: Ctrl+Y to scrollback, --continue opens the newest, full scrolling

- **Completed:** 2026-09-27T10:24:58-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `7039c51` (`land/review-20260927-r33`)
- **Notes:** In fullscreen iface, Ctrl+Y writes the transcript to native scrollback and --continue opens the newest session
- **Commits:** iface: restore 0.1.169 Ctrl+Y, --continue and transcript scrolling
- **Pins:** `apps/iface/tests/tui_pty.rs::ctrl_y_writes_the_fullscreen_transcript_to_native_scrollback`

### Window focus stops launching provider CLIs; Codex listing reused for a minute

- **Completed:** 2026-09-27T10:05:51-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `4dcdf8b` (`land/review-20260927-r29`)
- **Notes:** Focusing the window with unchanged provider binaries spawns no probe; Refresh still re-probes
- **Commits:** Keep a provider probe for minutes and stop probing on window focus; Reuse a complete Codex listing for a minute; explicit Refresh lists afresh

### Usage reads once a minute after turns; live logs compact to 90%; drag refits throttled

- **Completed:** 2026-09-27T10:02:33-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `bfa5767` (`land/review-20260927-e18`)
- **Notes:** A 24-frame splitter drag beside four terminals refits 12 times, not 92
- **Commits:** Read plan usage at most once a minute after finished turns; Compact live logs to 90% so streaming at the cap stays incremental; Throttle terminal refits while a divider is dragged

### Ambiguous sends are never offered for retry; Make orchestrator keeps the lead's approvals

- **Completed:** 2026-09-27T10:01:01-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `610c098` (`land/review-20260927-e17`)
- **Notes:** A Claude or Codex send whose delivery is unknown shows Resend/Discard instead of a retry
- **Commits:** Ambiguous sends are never offered for retry; host error codes reach the renderer; Make orchestrator no longer loosens approvals; the dead team launch is gone; Gate reads dispatcher.rs for the Electron suite too
- **Pins:** `electron/src/main/rust-ipc-bridge-error-shape.test.ts::classifies a pre-change rejection, without the shape, as unknown`

### ACP approvals and Stop reach a parked turn; iface exec and composer stay fail-closed

- **Completed:** 2026-09-27T09:55:09-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `1b57f0e` (`land/review-20260927-r31`)
- **Notes:** Answering or stopping a Grok/Qwen/ACP permission prompt works while its send is in flight
- **Commits:** Let approvals and Stop reach a parked ACP turn; close an overflowed session; iface: honest exec outcomes and no lost prompts on refusals
- **Pins:** `crates/interface-runtime/tests/runtime.rs::approval_and_cancel_reach_a_send_parked_on_its_turn`

### One basename, glyph set, chrome observer and usage-tone rule; Composer CSS declared once

- **Completed:** 2026-09-27T09:52:16-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `517d244` (`land/review-20260927-e12x`)
- **Notes:** One basename, context-percent, coordinator and usage-tone rule in the renderer; One glyph set and one chrome observer for the transcript and subagent panel; CLI tool rows share one rule; Composer.css declares each selector once

### Two flaky Rust tests fixed: PTY drain in sleep tests, Codex reap waits for the fake

- **Completed:** 2026-09-27T09:50:26-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `ce316f4` (`land/review-20260927-ptyfix`)
- **Notes:** Drain the PTY in the workspace-sleep process tests so the gate cannot hang; Wait for the fake Codex to start in the reap test instead of 750 ms

### Terminal port handshake in its own tested controller; typed preload channels; tested dev scripts

- **Completed:** 2026-09-27T08:58:54-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `eebbcbc` (`land/review-20260927-e13`)
- **Notes:** Terminal port handshake in its own tested controller; typed preload channels; tested dev scripts

### Collaboration MCP server with one connection struct; one UTF-8 clipper

- **Completed:** 2026-09-27T08:58:32-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `badcfb1` (`land/review-20260927-r22`)
- **Notes:** Pin the collaboration MCP server's frames before refactoring; One connection struct and one spawn arm in the collaboration MCP server; One byte clipper; no panicking lookups in the Away cycle; one id rule

### One push-channel set and turn-ended rule; terminal activity rules shared with Rust

- **Completed:** 2026-09-27T08:55:39-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `e860858` (`land/review-20260927-e11`)
- **Notes:** xterm's own protocol replies no longer clear a pane's waiting state; both suites read one activity fixture
- **Commits:** Move the terminal port handshake out of main.ts with behavior tests; Build preload channel strings from their keys; one encoder registration style; Run the dev supervisor's and smokes' decisions in tests, not by grepping them; List every retained, renderer-uncalled channel; One push-channel set, one appearance validator, a tested TUI fence; One turn-ended predicate for the queue, approvals and history; Share terminal activity rules with Rust; xterm replies stop clearing waiting; Hold the Rust terminal activity rules to the shared fixture
- **Pins:** `electron/src/renderer/terminal-activity.test.ts::distinguishes focus protocol reports from actual terminal input`

### Collaboration rules as one statement each; one drift check and effect ladder

- **Completed:** 2026-09-27T08:50:25-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `7fb8325` (`land/review-20260927-r21`)
- **Notes:** Pin collaboration record bytes and first-failure messages before refactoring; Write each collaboration validation rule as one invalid_if! statement; One approval-drift check and one effect ladder in the collaboration coordinator

### Worktree creation in phases; one project walk for Quick Open and search

- **Completed:** 2026-09-27T08:02:11-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `4f58c94` (`land/review-20260927-r20`)
- **Notes:** Pin worktree refusals, registry bytes and walk budgets before refactoring; Split worktree creation into phases; one ordered-map serializer; Call Unix metadata methods directly in storage; One project walk for Quick Open and search; one parent-segment helper

### Contracts goldens, one prompt-context validator, one command table; preview grants only owning roots

- **Completed:** 2026-09-27T07:46:05-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `bcb12b7` (`land/review-20260927-r19`)
- **Notes:** Pin the command catalog and prompt-context validation JSON; One struct for prompt-context validation and one command table; One function per prompt-context item and a plain estimate floor; One numstat walker, one discovery refusal path, tests out of the source tree; Output preview grants only roots that contain the target

### Packaged app ships only electron-updater at runtime; updater loads after the window

- **Completed:** 2026-09-27T07:40:59-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `35bee38` (`land/review-20260927-e7`)
- **Notes:** npm ls --omit=dev lists only electron-updater; background update checks never show an error box
- **Commits:** Ship only electron-updater in the packaged app; Load the updater after the window and settle background update checks; Reinstall Interface Dev when another checkout installed it

### Runtime lifecycle and reducer admission helpers; runtime.rs split; a flaky discovery test fixed

- **Completed:** 2026-09-27T07:28:22-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `ad9f079` (`land/review-20260927-r18`)
- **Notes:** One lifecycle helper and one metadata projection in the runtime; Shared admission helpers in the orchestration reducer; One disabled path in the updater's startup decision; Split the runtime's cache writer, event pump and tests out of runtime.rs

### One owner for each terminal stream's output; stream leaks on dismiss fixed

- **Completed:** 2026-09-27T07:14:44-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `2394cba` (`land/review-20260927-e16`)
- **Notes:** A dismissed terminal leaves no output cursor, sleeping screen or native worker entry behind
- **Commits:** One owner for each terminal stream's output state; Launch profile ids come from one list
- **Pins:** `electron/src/renderer/terminal-stream-router.test.ts::ends every stream of a removed terminal and drops its later output`

### Split the longest Codex, ACP and Claude session functions

- **Completed:** 2026-09-27T07:05:00-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `49b1d9e` (`land/review-20260927-r17`)
- **Notes:** Split the long Codex, ACP and Claude session functions

### Codex and ACP share one transport and session core; workers share one helper set

- **Completed:** 2026-09-27T06:53:06-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `89b53ac` (`land/review-20260927-r16`)
- **Notes:** One transport and one session core for Codex and ACP; OpenCode and Antigravity workers use the shared worker helpers; One checked transcript read and one blocking-store helper; Shared collaboration test fakes; no fixed sleep in a Codex exit test; Keep the shared transport's internals private

### Chat panes split into transcript and composer: typing no longer redraws the transcript

- **Completed:** 2026-09-27T06:50:16-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `eb0cda8` (`land/review-20260927-e15`)
- **Notes:** Five keystrokes in a chat pane render the composer only; streamed tokens never render the composer
- **Commits:** Move pure model code out of Composer, EffortBar and Conversation; Split the conversation pane so a keystroke redraws only its composer; Share one drop zone, image paste and slash menu between both prompt boxes; Split conversation.ts and viewmodels.ts into one module per rule set

### Shared lock, uid, timestamp, digest and path helpers replace per-crate copies

- **Completed:** 2026-09-27T06:20:42-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `b4d88d0` (`land/review-20260927-r15`)
- **Notes:** Share the helpers every crate kept its own copy of; One credential-name check for the gate cache and navigation

### One commit protocol per store crate; a write that reached nothing no longer locks a store

- **Completed:** 2026-09-27T05:58:42-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `d858562` (`land/review-20260927-r14`)
- **Notes:** An oversized harness result is refused and the next save succeeds; drift still closes the store
- **Commits:** Pin the bytes every settings and collaboration store writes; One commit protocol for the settings stores; One commit protocol for the collaboration snapshot stores; Shorter desktop store wrappers; one roster merge per save; Test that only a pre-rename failure counts as refused
- **Pins:** `crates/interface-settings/src/harness.rs::an_oversized_result_is_refused_and_later_records_still_save`

### App.tsx loses 841 lines: send lane, settings tokens, history paging, orchestration, inspector

- **Completed:** 2026-09-27T05:50:39-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `fcede82` (`land/review-20260927-e14`)
- **Notes:** Move the provider send lane and steering queue into a tested OutboundLane; Give the settings attempt-token protocol one owner; Move history paging and event batching out of App; Settings reads its customization inventory through an API prop; Move orchestration state and the canvas intents out of App; Move the task harness and inspector wiring out of App; Keep the removed-resume guard on the outbound lane file

### Remove Rust seams only tests reached: roster-only session binding, nested-process hook

- **Completed:** 2026-09-27T05:26:08-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `06f047f` (`land/review-20260927-r13`)
- **Notes:** Drop the roster-only provider session seam nothing calls; Drop the nested-process hook the terminal service never calls; Drop test-only collaboration decoder and workers models()

### iface worker broker and agent socket survive bad peers, oversize replies and turn races

- **Completed:** 2026-09-27T05:11:14-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `6362304` (`land/review-20260927-r11`)
- **Notes:** A dropped client before accept leaves the workers broker serving; an oversized agent reply gets an error frame
- **Commits:** Workers broker keeps serving past a dropped peer; its lead gate is tested; Agent socket answers oversized replies; queued sends re-check the turn
- **Pins:** `apps/iface/src/workers/transport_tests.rs::a_peer_that_hangs_up_before_accept_does_not_stop_the_broker`

### Remove unreachable terminal kill, rename, restart and duplicate paths

- **Completed:** 2026-09-27T05:08:30-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `3d62c9a` (`land/review-20260927-e10`)
- **Notes:** Drop a dead agent panel prop, a copied edge mapping and a test-only eof event

### Hidden notices stay hidden; control desk survives blocked storage; unsafe reopens are not retried

- **Completed:** 2026-09-27T04:56:23-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `f6cfe0f` (`land/review-20260927-e9`)
- **Notes:** The Fast toggle notice is read aloud but not drawn; a non-transient reopen refusal goes straight to the shell fallback
- **Commits:** Small renderer correctness fixes
- **Pins:** `electron/src/renderer/terminal-reopen-retry.test.ts::does not retry a refusal whose code is not transient, and goes straight to the shell fallback`

### Gate receipts per suite, timed steps, a lighter pin guard and a current terminal smoke

- **Completed:** 2026-09-27T04:49:10-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `787ed6a` (`land/review-20260927-r9`)
- **Notes:** Pin guard reads only the pinned files a push changes; Gate prints each step's time and each receipt keeps its own suite's; Each gate receipt is keyed by what its own suite reads; Terminal smoke expects OpenCode's deliberate --auto launch

### Launch in the saved theme without a light repaint

- **Completed:** 2026-09-27T04:39:17-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `3212696` (`land/review-20260927-e6`)
- **Notes:** Launching in a dark or palette theme paints that theme first, with one terminal re-theme
- **Pins:** `electron/src/renderer/appearance.test.ts::paints ${theme} before mount and never repaints anything else on the way to the confirmed read`

### Agent page, remote access and phone polls pause while hidden or settled

- **Completed:** 2026-09-27T04:21:50-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `333c0f4` (`land/review-20260927-e5`)
- **Notes:** A hidden agent page and a closed Settings make no work or remote-access reads
- **Commits:** Poll the agent page only while it is on screen; Read remote access status only while Settings shows; Ask the Mac about phone approvals once output settles

### worktree-prune keeps the Dev and promote checkouts; exact gate scope and pin spans

- **Completed:** 2026-09-27T04:14:00-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `31987ea` (`land/review-20260927-r8`)
- **Notes:** cargo xtask worktree-prune (dry run) lists ~/Developer/.interface-dev and .interface-promote as kept
- **Commits:** Keep xtask safe around the Dev checkout and make its gate and guards exact; Keep the promote driver checkout out of worktree-prune
- **Pins:** `xtask/src/worktree_prune.rs::the_promote_driver_is_kept_as_itself`

### Electron gate in about 100 s: pooled renderer lanes with per-lane deadlines

- **Completed:** 2026-09-27T03:59:32-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `f77d876` (`land/review-20260927-e8`)
- **Notes:** Run the renderer gate lanes side by side, each with its own deadline

### Control desk, collaboration and Away polls copy only what they return

- **Completed:** 2026-09-27T03:44:58-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `b2413ab` (`land/review-20260927-r7`)
- **Notes:** The control-desk view is byte-identical to the old clone-then-null view
- **Commits:** Copy only what control-desk, collaboration and Away reads return; Borrow agent records on lookups and probe each picker folder once

### Terminal requests stay off the lifecycle lock and disk writes; worktree ledger off async workers

- **Completed:** 2026-09-27T03:34:31-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `9955156` (`land/review-20260927-r6`)
- **Notes:** A terminal request returns while a lifecycle publish is stalled in its durable write
- **Commits:** Keep terminal requests off the lifecycle lock and desktop gate; Write and scan the managed-worktree ledger off async workers; Serve orchestration snapshots without waiting for a commit

### iface TUI stays live while providers start, send, stop and resume

- **Completed:** 2026-09-27T03:05:25-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `213eb05` (`land/review-20260927-r5`)
- **Notes:** Ctrl+C during a slow provider start in iface exits at once; a second Enter during a send sends once
- **Commits:** Keep the iface TUI live while providers start, send, stop and resume; Look up iface provider commands through the host's login-shell probe
- **Pins:** `apps/iface/tests/tui_pty.rs::ctrl_c_during_a_slow_provider_start_exits_without_waiting_for_it`

### Sidebar, palette and open Browsers redraw only rows that changed

- **Completed:** 2026-09-27T02:47:29-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `51a9511` (`land/review-20260927-e4`)
- **Notes:** A streamed token or rename redraws no other sidebar row; palette Enter runs the highlighted command after a push
- **Commits:** Redraw only the sidebar and palette rows that changed; Stop re-rendering the app for clicks and focus that change nothing; Stop redrawing open Browsers when another terminal changes

### Busy Codex/ACP sessions survive output bursts; claude after codex in a pane is captured

- **Completed:** 2026-09-27T02:23:40-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `1a16134` (`land/review-20260927-r10`)
- **Notes:** A burst of 40+ small frames while a Codex session is busy is delivered in order instead of failing the session
- **Commits:** Keep busy Codex and ACP sessions alive through output bursts
- **Pins:** `crates/engine-codex/src/transport.rs::a_burst_while_the_session_is_busy_waits_instead_of_failing_the_transport`

### Record the cleanup review and its landing lanes

- **Completed:** 2026-09-27T02:08:35-05:00
- **Kind:** record
- **Release:** v0.3.142
- **Commit:** `16b2876` (`record/review-20260927-todo`)
- **Notes:** Record the cleanup review and its landing lanes

### Terminal panes redraw only what changed: titles, activity and splitter drags

- **Completed:** 2026-09-27T02:07:51-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `a5e5a95` (`land/review-20260927-e3b`)
- **Notes:** A background pane's title change or a splitter step redraws no other pane or emulator
- **Commits:** Redraw only the terminal panes and emulators that changed; Wait for the cursor fixture's screens to lay out before clicking

### Provider CLIs are asked their version once per install; consultations probe one provider

- **Completed:** 2026-09-27T02:03:20-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `18de876` (`land/review-20260927-r3`)
- **Notes:** A second provider probe with the same Claude or Codex binary spawns no --version process
- **Commits:** Ask provider CLIs for their version once per installed binary; Probe only the consulted provider before an agent consultation; Let idle spare Codex history children exit; Fence the single-provider listing refresh like the full refresh; Report a kept-version CLI that no longer starts as unavailable

### Terminal cursor fixture waits for layout before clicking (fixes a flaky gate lane)

- **Completed:** 2026-09-27T01:58:58-05:00
- **Kind:** internal
- **Release:** v0.3.142
- **Commit:** `f9c4c8f` (`land/fixture-cursor-race`)
- **Notes:** Wait for the cursor fixture's screens to lay out before clicking

### Window focus no longer rewrites the session registry when nothing changed

- **Completed:** 2026-09-27T01:31:17-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `1b34694` (`land/review-20260927-r2`)
- **Notes:** Focusing the window with no new sessions leaves registry.json's revision, inode and bytes unchanged
- **Commits:** Leave the registry cache alone when a provider listing changes nothing; Read stored records once per write and sync restored folders; Skip rewriting unchanged worker, Away and telemetry files

### Agent tool calls skip ps walks; sleeping panes and the budget watch wake less

- **Completed:** 2026-09-27T01:19:26-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `5cfee06` (`land/review-20260927-r4`)
- **Notes:** A Claude or Codex tool call in a pane with no waiting worker result runs no ps process
- **Commits:** Skip the worker notice's ps walk unless a pane's conversation has a result; Poll sleeping panes less and check shell labels outside the state lock; Let the Codex session hook read its own parent without ps; Sleep the agent budget watch while no task is running

### Streaming chats redraw only what changed: rows, markdown and hidden panes

- **Completed:** 2026-09-27T01:12:30-05:00
- **Kind:** speed
- **Release:** v0.3.142
- **Commit:** `bb0ac93` (`land/review-20260927-e1e2`)
- **Notes:** A long tool-heavy chat streams without re-rendering its folded runs; markdown parses only the growing tail
- **Commits:** Stream conversation tokens without redrawing settled rows; Stream markdown, subagent details and hidden panes without per-token rework

### Agent store errors no longer close chats; fewer locks and thinking signals per token

- **Completed:** 2026-09-27T00:44:46-05:00
- **Kind:** fix
- **Release:** v0.3.142
- **Commit:** `9d1248c` (`land/review-20260927-r1`)
- **Notes:** An unreadable agent store leaves a plain Codex chat open, and a reasoning span sends one thinking signal
- **Commits:** Keep agent storage failures to agent sessions and off the event path; Send one thinking signal per reasoning span
- **Pins:** `apps/interface-host/src/agents_tests.rs::an_unreadable_agent_store_never_closes_an_ordinary_chat`

### Pane + drops Canvas; Browser opens in a pane; iOS Simulator starts an iPhone

- **Completed:** 2026-09-27T00:17:02-05:00
- **Kind:** feature
- **Release:** v0.3.142
- **Commit:** `1194f46` (`fix/new-pane-menu-20260926`)
- **Notes:** Pane + has no Canvas; Browser opens your running dev server in a pane beside the terminals; iOS Simulator opens on an iPhone
- **Commits:** Pin guard reads JavaScript regex literals as literals; In-app Browser pane, a working iOS Simulator, and no Canvas in the pane +
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::opens a New pane menu from the + with tools, launchers and only real chords`, `electron/src/main/browser-pane.test.ts::pins a webview's preferences whatever its markup asked for`, `crates/interface-terminal/src/workspace_tools.rs::prefers_a_booted_iphone_then_the_last_used_then_the_newest_runtime`

### Record the 0.3.141 phone release

- **Completed:** 2026-09-26T22:51:13-05:00
- **Kind:** record
- **Release:** v0.3.142
- **Commit:** `9410a57` (`docs/phone-0.3.141-20260926`)
- **Notes:** Record the 0.3.141 phone release

### Chat no longer lists Away work and review sessions

- **Completed:** 2026-09-26T22:43:34-05:00
- **Kind:** fix
- **Release:** v0.3.141
- **Commit:** `d257bf9` (`fix/chat-hide-away-runs-20260926`)
- **Notes:** Chat's session list has no 'Away work' or 'Away review' rows; the Away panel still opens them
- **Commits:** Keep Away run sessions out of the Chat list
- **Pins:** `electron/src/renderer/away-view.test.ts::lists the sessions each Away turn created and skips turns still launching`

### iPhone reaches the Mac over Tailscale and keeps its routes current

- **Completed:** 2026-09-26T22:37:06-05:00
- **Kind:** feature
- **Release:** v0.3.141
- **Commit:** `8a3a65f` (`feat/phone-tailscale-route-20260926`)
- **Notes:** With Tailscale on both devices the paired iPhone connects on cellular and shows Connected through Tailscale
- **Pins:** `crates/interface-contracts/src/remote_connection.rs::mac_routes_add_tailnet_addresses_without_widening_the_qr`, `apps/interface-ios/MacAddressTests.swift::testATailnetRouteFromTheMacConnectsAndNamesItself`

### iPhone session list grouped by project, matching the Mac sidebar

- **Completed:** 2026-09-26T22:26:27-05:00
- **Kind:** feature
- **Release:** v0.3.140
- **Commit:** `23fbaa8` (`feat/phone-projects-20260926`)
- **Notes:** The iPhone lists sessions under the same projects as the Mac sidebar, with Other last
- **Commits:** iPhone session list grouped by project; iPhone projects match the Mac sidebar; Keep the encrypted gateway test within the line limit
- **Pins:** `electron/src/remote/projects.test.ts::files each terminal under the same project as the Rust phone roster`, `crates/interface-remote/src/workspaces.rs::files_terminals_under_the_same_projects_as_the_mac_sidebar`, `apps/interface-ios/MacAddressTests.swift::testRosterV3FilesSessionsLikeTheMacSidebar`

### iPhone pairing works on a restored or transferred iPhone

- **Completed:** 2026-09-26T22:06:04-05:00
- **Kind:** fix
- **Release:** v0.3.140
- **Commit:** `04e96e8` (`fix/phone-restored-identity-20260926`)
- **Notes:** On an iPhone set up from an older one, scanning a Mac pairing code shows a verification number instead of 'no longer approved'
- **Pins:** `apps/interface-ios/MacAddressTests.swift::testAMissingIdentityIsNotAMacRefusal`

### iPhone app refresh: provider marks, reply-bar glyph fix, phone 0.3.2

- **Completed:** 2026-09-26T21:53:29-05:00
- **Kind:** look
- **Release:** v0.3.140
- **Commit:** `9daadb6` (`feat/phone-refresh-20260926`)
- **Notes:** Phone session rows and the reply bar show each provider's mark; Safari's Add to Home Screen uses the Fold icon
- **Commits:** iPhone app refresh: provider marks, reply-bar glyph, 0.3.2; Pin the phone touch icon route and the phone page's codicons
- **Pins:** `crates/interface-remote/src/http.rs::serves_the_touch_icon_and_refuses_a_build_without_it`, `electron/src/remote/codicons.test.ts::names only codicons the vendored subset draws`

### Terminal text keeps its bold GPU weight after sleep or a display change

- **Completed:** 2026-09-26T21:21:31-05:00
- **Kind:** fix
- **Release:** v0.3.140
- **Commit:** `7b5ba63` (`fix/terminal-webgl-recovery-20260926`)
- **Notes:** Pane text stays bold after sleep/wake or a display change instead of turning thin until a workspace switch
- **Commits:** Terminal text keeps its GPU weight after a lost context
- **Pins:** `electron/src/renderer/terminal-webgl-pool.test.ts::hands a freed slot to a visible pane left on the DOM renderer`, `electron/src/renderer/terminal-webgl-pool.test.ts::backs off, gives up, and starts over once a renderer has stayed up`, `electron/src/renderer/terminal-webgl-pool.test.ts::brings a visible pane back to WebGL instead of leaving its text thin`

### Original pane buttons: smaller, thicker +, ···, expand and close

- **Completed:** 2026-09-26T21:00:59-05:00
- **Kind:** look
- **Release:** v0.3.139
- **Commit:** `80a2fea` (`fix/pane-buttons-20260926`)
- **Notes:** Pane header buttons (···, expand, +, close) are small 12px glyphs with a heavier stroke again
- **Commits:** Restore the compact, heavier pane header buttons
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps tabs mounted in explicit tabpanels and labels close as detach-only`, `electron/src/renderer/components/AgentTile.test.ts::keeps the pane controls visible and offers a split`

### Compact top bar: Layout, Agent overview, Usage and Attention in ···

- **Completed:** 2026-09-26T14:04:24-05:00
- **Kind:** look
- **Release:** v0.3.139
- **Commit:** `0561f38` (`feat/compact-toolbar-20260926`)
- **Notes:** The top bar shows only Outputs, Voice, Away and ···; ··· lists Attention, Usage and the layout controls
- **Pins:** `electron/src/renderer/components/AttentionTray.test.ts::draws no bell of its own; the ··· button carries its count and says it`, `electron/src/renderer/components/TerminalPrimitives.test.ts::keeps the reference layout rows, now inside the window's ··· menu`, `electron/src/renderer/workspace-ui.test.ts::shows the update pill beside the terminal controls only while an update exists`

### Porcelain: the new default look

- **Completed:** 2026-09-26T12:38:28-05:00
- **Kind:** look
- **Release:** v0.3.139
- **Commit:** `0a4b7ae` (`feat/porcelain-20260926`)
- **Notes:** Panes are light frames around dark screens; Settings > Color theme lists Porcelain and Porcelain Night
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::draws a rounded selection box around the focused terminal pane`, `electron/src/renderer/theme-accessibility.test.ts::keeps terminal body, idle and focused header tiers distinct in every palette`

### Agent permission modes: Ask, provider Auto or Bypass per agent

- **Completed:** 2026-09-26T09:51:00-05:00
- **Kind:** feature
- **Release:** v0.3.139
- **Commit:** `cfce6be` (`feat/agent-permission-modes-20260925`)
- **Notes:** Agent Advanced settings offer Ask, Auto or Bypass; Auto and Bypass agents run in Claude's or Codex's own mode and send what they escalate to you
- **Commits:** Spec agent permission modes: gated, auto and bypass; Plan agent permission modes; Contract agent permission modes and agent launch access (protocol 32); Store agent permission modes beside the roster; Save agent permission modes through agent control; Launch Claude agents in auto or bypass mode; Launch Codex agents with auto review or full bypass; Refuse agent access for ACP providers; Launch and route agents by their permission mode; Choose an agent's permissions in its Advanced settings; Document agent permission modes; Send a Claude agent's escalations to the owner in Auto and Bypass; Route agent approvals by the mode the session launched with; Say when an agent's new permission mode applies; Clean up a removed agent's work before its permission mode; Format agent permission mode tests; Let the startup-download fixture test accept later protocols
- **Pins:** `crates/engine-claude/src/adapter_tests.rs::agent_access_sessions_leave_every_escalation_to_interface`, `crates/engine-codex/src/adapter_tests.rs::agent_access_threads_use_codex_auto_review_or_full_bypass`, `apps/interface-host/src/agents_tests.rs::approvals_follow_the_mode_the_live_session_launched_with`, `crates/interface-settings/src/agents/tests.rs::permission_modes_live_beside_the_roster_and_follow_the_agent`
- **Supersedes:** c8e9135 — host protocol v32 (agent permission modes) makes the exact-v31 version assertion impossible; the test now requires v31 or newer and still verifies the same startup-download projection and notice.

### Record staged Claude restart repair

- **Completed:** 2026-09-25T22:10:18-05:00
- **Kind:** record
- **Release:** v0.3.139
- **Commit:** `05b70fd` (`fix/claude-reopen-20260925`)
- **Notes:** Record verified 0.3.138 staging and production acceptance boundary

### Reconcile Claude fix release into Dev

- **Completed:** 2026-09-25T22:08:32-05:00
- **Kind:** internal
- **Release:** v0.3.139
- **Commit:** `2920b0a` (`fix/claude-reopen-20260925`)
- **Notes:** Claude conversations reopen after a project moves; chore(release): bump Interface to 0.3.138

### Record Claude restart fix release readiness

- **Completed:** 2026-09-25T21:27:48-05:00
- **Kind:** record
- **Release:** v0.3.138
- **Commit:** `8a89bce` (`fix/claude-reopen-20260925`)
- **Notes:** Record verified Claude restart fix and production release boundary

### Claude conversations reopen after a project moves

- **Completed:** 2026-09-25T21:26:25-05:00
- **Kind:** fix
- **Release:** v0.3.138
- **Commit:** `cbedd8f` (`fix/claude-reopen-20260925`)
- **Notes:** Restart Interface after moving a project; saved Claude panes reopen their original conversations.
- **Commits:** Recover Claude pane receipts through moved project aliases
- **Pins:** `apps/interface-host/src/claude_terminal_session_tests.rs::moved_project_receipt_recovers_on_restart_and_unblocks_the_saved_pane`

### Blue dot when an agent is done

- **Completed:** 2026-09-25T18:18:35-05:00
- **Kind:** look
- **Release:** v0.3.137
- **Commit:** `0919a3a` (`feat/done-dot-20260925`)
- **Notes:** An agent that finishes while you're in another pane shows a blue dot until you focus it
- **Pins:** `electron/src/renderer/components/TerminalPrimitives.test.ts::paints a finished, unseen agent turn blue until something outranks it`, `electron/src/renderer/components/Sidebar.test.ts::paints a workspace blue when an agent finished a turn nobody has looked at`, `electron/src/renderer/terminal-activity.test.ts::marks a finished turn in a pane nobody is looking at, never the focused one`

### Explain panes waiting on iCloud startup files

- **Completed:** 2026-09-25T15:15:51-05:00
- **Kind:** fix
- **Release:** v0.3.137
- **Commit:** `109f6bf` (`fix/icloud-startup-wait-20260925`)
- **Notes:** A new pane in an iCloud project with cloud-only git or CLAUDE.md files shows 'Waiting for cloud download: …' until they download, not a blank pane
- **Commits:** Say when a pane waits on cloud-only startup files
- **Pins:** `crates/interface-terminal/tests/suite/embedded_terminal.rs::a_pane_names_an_evicted_startup_file_until_it_downloads`, `crates/interface-terminal/src/startup_files.rs::a_git_directory_fetches_head_config_and_then_the_branch_ref`, `electron/src/renderer/terminal-startup-download.test.ts::matches the Rust projection and its notice`

### Launch security fixes: scoped agent reads, sandboxed Claude Away, confined pane/phone access

- **Completed:** 2026-09-25T14:50:07-05:00
- **Kind:** fix
- **Release:** v0.3.137
- **Commit:** `c801a21` (`fix/security-sweep-20260925`)
- **Notes:** Agents ask before reading outside their folders; Claude Away runs sandboxed; read_pane and the phone listener refuse outside callers
- **Commits:** Keep automatic agent reads inside granted folders; Run Claude Away inside Claude Code's OS sandbox; Confine read_pane to the caller's pass-off edge; Reveal agent folders instead of opening them; Answer only local-network peers on the phone listener; Take patched rustls and vitest releases; Close the renderer CSP directives default-src leaves open; Record the launch security items that need the owner
- **Pins:** `crates/interface-desktop-services/src/agent_policy.rs::policy_table`, `crates/engine-claude/src/adapter_tests.rs::workspace_write_launches_claude_in_its_os_sandbox`, `crates/interface-collaboration/src/project_away/tests.rs::claude_runs_sandboxed_without_bypass_and_codex_keeps_its_sandbox`, `crates/interface-remote/src/http/direct_tests.rs::only_local_network_peers_reach_the_nearby_listener`

### Every release lists what it ships and leaves out

- **Completed:** 2026-09-24T17:42:18-05:00
- **Kind:** feature
- **Release:** v0.3.137
- **Commit:** `b92414d` (`feat/release-contents-20260924`)
- **Notes:** promote --dry-run prints what ships, what stays on Dev, and what it replaces
- **Commits:** Listing what stays on Dev never fails a staged release; Record the release contents landing
- **Pins:** `xtask/src/landings.rs::release_summary_groups_user_facing_landings_and_counts_the_rest`, `xtask/src/closeout.rs::closeout_records_what_a_landing_is_and_guards_shipped_pins`

### Design for release contents and shipped pins

- **Completed:** 2026-09-24T17:18:30-05:00
- **Kind:** record
- **Release:** v0.3.137
- **Commit:** `99b1882` (`docs/release-contents-20260924`)
- **Notes:** Design for release contents and shipped pins

### Record the icon review handoff

- **Completed:** 2026-09-24T16:13:05-05:00
- **Kind:** feature
- **Release:** v0.3.137
- **Commit:** `edbb49d` (`feat/icon-review-20260924`)
- **Notes:** Record the icon review handoff

### Consistent icon weights and one glyph per meaning

- **Completed:** 2026-09-24T16:11:41-05:00
- **Kind:** feature
- **Release:** v0.3.137
- **Commit:** `c3207ec` (`feat/icon-review-20260924`)
- **Notes:** Consistent icon weights and one glyph per meaning

### Slide the sidebars and animate every floating surface

- **Completed:** 2026-09-24T15:44:45-05:00
- **Kind:** feature
- **Release:** v0.3.137
- **Commit:** `9977dc7` (`feat/sidebar-motion-20260924`)
- **Notes:** Slide the output preview in and out like the agent overview

### Record completed Agent production test

- **Completed:** 2026-09-24T15:23:54-05:00
- **Kind:** internal
- **Release:** v0.3.135
- **Commit:** `a8b1054` (`test/agent-production-sandbox-20260924`)
- **Notes:** Mark sandboxed production test complete and record Dev verification

### Sandboxed Agent production verification

- **Completed:** 2026-09-24T15:22:22-05:00
- **Kind:** internal
- **Release:** v0.3.135
- **Commit:** `a2955b3` (`test/agent-production-sandbox-20260924`)
- **Notes:** Isolate production package tests from owner state; Preserve Agent stop reasons and verify sandboxed release behavior; Permit read-only terminal identity checks in production sandbox tests; Record passing signed Agent production acceptance

### Preview project outputs beside agent terminals

- **Completed:** 2026-09-24T15:10:09-05:00
- **Kind:** feature
- **Release:** v0.3.135
- **Commit:** `7e66acc` (`feat/output-preview-20260924`)
- **Notes:** Add output previews and file-specific feedback

### Verify Keep Downloaded for Claude startup repair

- **Completed:** 2026-09-24T14:49:31-05:00
- **Kind:** fix
- **Release:** v0.3.135
- **Commit:** `c34d064` (`fix/claude-startup-hang-20260924`)
- **Notes:** docs: verify Finder pin for Claude startup repair

### Record Claude startup iCloud repair

- **Completed:** 2026-09-24T14:45:00-05:00
- **Kind:** fix
- **Release:** v0.3.135
- **Commit:** `a8edeff` (`fix/claude-startup-hang-20260924`)
- **Notes:** docs: record Claude startup iCloud repair

### Restore saved panes after startup retries

- **Completed:** 2026-09-24T12:00:12-05:00
- **Kind:** fix
- **Release:** v0.3.135
- **Commit:** `08916b4` (`fix/blank-saved-panes-20260924`)
- **Notes:** Keep saved pane reopen retries alive across workspace updates

### Blue terminal typing caret

- **Completed:** 2026-09-24T10:26:45-05:00
- **Kind:** fix
- **Release:** v0.3.134
- **Commit:** `c0ade9b` (`fix/reference-terminal-caret-20260924`)
- **Notes:** Match the terminal typing caret to the blue reference

### Record the 0.3.133 promotion

- **Completed:** 2026-09-24T09:52:47-05:00
- **Kind:** record
- **Release:** v0.3.134
- **Commit:** `430b8da` (`docs/promoted-0.3.133`)
- **Notes:** Record the 0.3.133 promotion in the to-do list

### Record Agent mode validation and Dev landing

- **Completed:** 2026-09-24T09:45:47-05:00
- **Kind:** feature
- **Release:** v0.3.133
- **Commit:** `82da35a` (`feat/agent-mode-audit-20260924`)
- **Notes:** Record Agent mode validation and Dev landing

### Reliable Agent tasks and calendar routines

- **Completed:** 2026-09-24T09:43:35-05:00
- **Kind:** feature
- **Release:** v0.3.133
- **Commit:** `30516fe` (`feat/agent-mode-audit-20260924`)
- **Notes:** Make Agent tasks durable, scoped and verifiable

### Record performance audit verification

- **Completed:** 2026-09-24T08:22:03-05:00
- **Kind:** record
- **Release:** v0.3.133
- **Commit:** `3fce3a9` (`improve/performance-audit-20260924`)
- **Notes:** Record completed audit verification and Dev landing

### Performance audit fixes and controller cleanup

- **Completed:** 2026-09-24T08:19:53-05:00
- **Kind:** speed
- **Release:** v0.3.133
- **Commit:** `48bae58` (`improve/performance-audit-20260924`)
- **Notes:** Implement performance audit fixes and controller boundaries

### Saved panes reopen after their folder moves

- **Completed:** 2026-09-24T07:13:07-05:00
- **Kind:** fix
- **Release:** v0.3.133
- **Commit:** `bf96971` (`fix/reopen-moved-folder`)
- **Notes:** Saved panes reopen after their folder moves

### Faster workspace switching

- **Completed:** 2026-09-23T23:03:58-05:00
- **Kind:** fix
- **Release:** v0.3.133
- **Commit:** `cf45137` (`fix/workspace-switch-latency-20260923`)
- **Notes:** Reuse bounded terminal GPU renderers across workspace switches

### Note symlinked-project terminal fix is ready to promote

- **Completed:** 2026-09-23T22:23:39-05:00
- **Kind:** record
- **Release:** v0.3.133
- **Commit:** `612af01` (`docs/symlink-launch-ready`)
- **Notes:** Note symlinked-project terminal fix is ready to promote

### Record the thin terminal cursor as ready to promote

- **Completed:** 2026-09-23T22:23:10-05:00
- **Kind:** record
- **Release:** v0.3.133
- **Commit:** `e00618a` (`docs/caret-ready-20260923`)
- **Notes:** Record the thin terminal cursor as ready to promote

### Launch terminals in projects moved behind a symlink

- **Completed:** 2026-09-23T22:20:58-05:00
- **Kind:** fix
- **Release:** v0.3.133
- **Commit:** `8881d95` (`fix/symlinked-workspace-launch`)
- **Notes:** Launch terminals in projects moved behind a symlink

### Thin caret is the default terminal cursor

- **Completed:** 2026-09-23T22:17:43-05:00
- **Kind:** fix
- **Release:** v0.3.133
- **Commit:** `6e2399f` (`fix/default-caret-20260923`)
- **Notes:** Thin caret is the default terminal cursor

### Project and Dev checkout out of iCloud

- **Completed:** 2026-09-23T20:21:11-05:00
- **Kind:** internal
- **Release:** v0.3.132
- **Commit:** `c5ef15b` (`chore/project-off-icloud`)
- **Notes:** Move the project and Dev checkout out of iCloud

### Thinner, shorter bar cursor in terminal panes

- **Completed:** 2026-09-23T17:05:37-05:00
- **Kind:** fix
- **Release:** v0.3.132
- **Commit:** `ba4f931` (`fix/smaller-terminal-cursor`)
- **Notes:** Thinner, shorter bar cursor in terminal panes

### Close the agent overview entry after its 0.3.131 promotion

- **Completed:** 2026-09-23T14:48:19-05:00
- **Kind:** record
- **Release:** v0.3.132
- **Commit:** `2c383ae` (`docs/agent-overview-accepted-20260923`)
- **Notes:** Close the agent overview entry after its 0.3.131 promotion

### Audit cleanup (host 2/3): readiness-based terminal I/O, one service ticker, journal off workers

- **Completed:** 2026-09-23T14:37:13-05:00
- **Kind:** internal
- **Release:** v0.3.132
- **Commit:** `3d556b6` (`land/audit-host-b2`)
- **Notes:** Spell out persisted orchestration error codes; derive permit count; Log why remote access is unavailable instead of failing silently; Read the hook's own parent and liveness without spawning ps or kill; Give host.sock request reads the client's 15 s deadline; Bound MCP socket lines and in-flight calls; unlink only our sockets; Kill the Antigravity usage probe's whole process group on timeout; Retire input lanes and replay streams with their terminals; Drive fd3/fd4 by readiness and encode frames outside the queue lock; Commit orchestration mutations on the blocking pool; Run collaboration journal transitions on the blocking pool; Read one phone draft or twenty results without cloning the harness; Parse only appended Codex index lines; back off missing prompts; Start the login-shell PATH probe before the stores open; Cheaper workspace-sleep checks and a backoff after a declined sleep; Drop back-to-back process listings from pane caller checks; Pin a renamed pane's title without holding the state lock; One service ticker for sleep, shell-display and Grok-title ticks; Give release_terminal_entries a per-entry function; drop a dead Option; Extract the process waiter and one teardown retry loop; Handle a reaped manual provider in one branch; Name the capture ownership guard, live state and capture reset once; Use is_live everywhere; a bare Mutex<bool> for the provider wake; Share the Claude capture lock release and trust-boundary parsing; Share the create/restart spawn exposure tail; Deduplicate uncommitted-terminal cleanup and back off its retries

### Audit cleanup (renderer 2/6): memo boundaries restored; renderer loads while the host starts

- **Completed:** 2026-09-23T14:30:11-05:00
- **Kind:** internal
- **Release:** v0.3.131
- **Commit:** `4d366d8` (`land/audit-renderer-b2`)
- **Notes:** Delete two phone conversation rules that never matched; Delete superseded delegation fixtures; document the HPKE vector generators; Run the CLI pane appearance matrix once per style, not three times; Read orchestration and collaborations once at launch, not twice; Forbid renderer workers in the CSP now that Monaco is gone; Read terminal bells once, from the PTY scanner only; Keep memo(IntegratedTerminalCenter) effective: hoist four inline props; Keep memo(Sidebar) effective: stable project and workspace pin handlers; Skip sticky-header clip measurement while the sidebar is solid; Keep memo(AgentCanvas) effective across streamed frames and pushes; Skip the App render for voice level frames that light the same bars; Stop recompacting a live log whose disposable history still fits; Keep main-thread streaming incremental after a subagent spawns; Memoize the transcript's tool, file-change and subagent rows; Render session panes through a memoized SessionTile; Keep AgentCanvas picker options stable across pan and drag renders; Stop rewriting the saved roster for resize-only terminal upserts; Keep the workspace switcher stable through splitter drags and focus; Poll pane delegation at 1 s only while workers run or a turn is live; Release a parked terminal's WebGL context and reload it when slotted; Poll the phone's conversation and output only when they can change; Load the renderer while the Rust host starts

### Audit cleanup (core 2/4): registry writes outside the lock; Away and ACP robustness fixes

- **Completed:** 2026-09-23T14:25:50-05:00
- **Kind:** internal
- **Release:** v0.3.131
- **Commit:** `2dff636` (`land/audit-core-b2`)
- **Notes:** Reject u64::MAX orchestration sequences instead of overflowing; Register for the ingress drain wakeup before checking the count; Bound the Claude fork parent walk so a parentUuid cycle ends; Open Claude transcripts with libc's O_NOFOLLOW, not magic numbers; Keep failed or oversize Claude history reads out of the LRU; Close Claude sessions against one shared deadline, aborting stragglers; Return Claude probe, usage and command answers before the CLI exits; Map Claude commands_changed through the shared command normalizer; Drop Claude context-usage refreshes that a newer value superseded; Shut one-shot ACP probe and usage agents down when the caller drops; Keep an approval answerable when its answer was definitely unsent; Decode ACP session/update params by moving fields, not via flatten; Share the Codex read-lane checkout and fail lanes on oversize frames; Clone and validate the orchestration snapshot once per apply; Answer missing project-root cwds with a stat, not a git spawn; Scan session-search pages on blocking threads inside each fetch task; Read collaboration history state and tails without whole-session copies; Write the registry cache after releasing the registry lock; Run the two registry cadence tests on tokio paused time; Document the registry save split in the area docs; Refuse a full Away history without marking storage uncertain; Keep the tail of long Away turn reports, where the result is; Let an owner's Away cycle Stop wait out the apply checkpoint; Delete the write-only collaboration audit_degraded flag; Stop re-driving collaborations that wait on a person per source token; Share one whole-registry push coalescer across the host's owners; Apply registry pushes with the event batch and keep unchanged rows; Format the merged orchestration start call

### Audit cleanup (services 2/5): iface transcript restyles only changes; batched review diffs

- **Completed:** 2026-09-23T14:12:16-05:00
- **Kind:** internal
- **Release:** v0.3.131
- **Commit:** `4ed2dd5` (`land/audit-services-b2`)
- **Notes:** Look up saved terminals without cloning the whole roster; Skip rewriting an unchanged terminal workspace roster; Stop cloning the agent store for reads, idle ticks and text deltas; Skip the ps ancestry probe for MCP notifications in the worker bridge; Probe log-writer liveness only past the retention window; Avoid login-shell PATH lookups for overrides and optional helpers; Read the registry, not every provider, for unseen collaboration keys; Run steer-history and config fsyncs on the blocking pool; Harden settings store modes only when they are not already private; Remove stale registry rows in one pass after a complete listing; Read the managed-worktree store once when opening it; Parse the on-disk JSON in compare_and_set only when it changed; Use one terminal snapshot per remote request; bound history envelopes; Read prompt-context roots once per operation and realpath only distinct ones; Downscale pasted-image previews on the native buffer; Prefilter editor roots lexically before verifying them; Keep terminal-state gate reads off async workers in the host; Batch Review patch diffs into one git call per layer; Slow the iPhone roster poll while a session view is open; Probe nearby phone routes together and pass reply bodies through; Define container and tar tool families once in the agent policy; Lay out iface's transcript incrementally with exact wrapped heights

### Record the agent overview's pending Dev acceptance

- **Completed:** 2026-09-23T13:51:08-05:00
- **Kind:** record
- **Release:** v0.3.131
- **Commit:** `b288753` (`docs/agent-overview-dev-check-20260923`)
- **Notes:** Record the agent overview's pending Dev acceptance

### Record the Claude activity landing awaiting the Dev refresh

- **Completed:** 2026-09-23T12:47:39-05:00
- **Kind:** fix
- **Release:** v0.3.131
- **Commit:** `332a8d9` (`fix/claude-background-activity`)
- **Notes:** Record the Claude activity landing awaiting the Dev refresh

### Claude panes stay working while background workflows run

- **Completed:** 2026-09-23T12:41:33-05:00
- **Kind:** fix
- **Release:** v0.3.131
- **Commit:** `abf3a6b` (`fix/claude-background-activity`)
- **Notes:** Keep Claude panes working while background workflows run

### Record the pending Dev refresh after the instruction landing

- **Completed:** 2026-09-23T11:57:30-05:00
- **Kind:** record
- **Release:** v0.3.131
- **Commit:** `4f0128c` (`docs/opus55-dev-refresh-handoff`)
- **Notes:** Record the pending Dev refresh after the instruction landing

### Agent overview beside the terminal panes

- **Completed:** 2026-09-23T11:04:24-05:00
- **Kind:** feature
- **Release:** v0.3.131
- **Commit:** `34450ef` (`feat/agent-overview-20260923`)
- **Notes:** Agent overview beside the terminal panes

### Instruction files tuned for Opus 5.5

- **Completed:** 2026-09-23T10:50:41-05:00
- **Kind:** record
- **Release:** v0.3.131
- **Commit:** `401fc20` (`docs/opus55-instructions`)
- **Notes:** Instruction files tuned for Opus 5.5

### Integrate approved 0.3.130 with current Dev

- **Completed:** 2026-09-23T10:32:54-05:00
- **Kind:** feature
- **Release:** v0.3.131
- **Commit:** `d17faf0` (`feat/new-project-from-workspaces-20260922`)
- **Notes:** Create projects from the Workspaces plus menu; Update packaged smoke for the Workspaces create menu; chore(release): bump Interface to 0.3.130

### Audit cleanup (host 1/3): no ps storms, batched terminal output, keystrokes off the lifecycle lock

- **Completed:** 2026-09-23T08:52:29-05:00
- **Kind:** internal
- **Release:** v0.3.131
- **Commit:** `15b69ad` (`cleanup/audit-host-20260923`)
- **Notes:** Replace the unavailable collaboration control with an Option; Drop the write-only live-handle generation from the projection; Remove the redundant LocalHost mutation gate; Inline start_with_services and the agent_is_terminal alias; Test session management through the library, not a #[path] copy; Collapse the bootstrap opener structs into one runtime core; Open the worktree ledger in its own bootstrap helper; Delete the unreachable non-unix fallbacks in the host; Remove test-only constructor layers from the host API; Implement supplemental usage on the OpenCode reader directly; Drop the unreachable persist_provider_session lifecycle hook; Remove dead state from the OSC title scanners; Stop copying native worker output into an unreadable tail; Answer empty worker notices before walking the process tree; Stop re-reading an unchanged foreground job every second; Skip rewriting unchanged launch overlays at boot; Open the report-worker store without an unconditional rewrite; Restore phone access without rewriting unchanged settings; Stop busy-polling blocked output pumps every 2 ms; Merge queued PTY output into one frame and drop the pump timeout; Serve workspace sleep from one ticker instead of a thread per pane; Run hook and wrapper modes before building the Tokio runtime; Keep the committed orchestration snapshot as the read memo; Serve the owner window's orchestration calls without serde copies; Move path probes in async handlers off the Tokio workers; Adapt the lane to origin/dev's storage trim and iface lints; Build each collaboration status once and probe models together; Collect each approved session root once, without cloning rows; Keep a finished login shell's PATH when a background job holds stdout; Release the host read buffer after a huge pasted-image request; Read saved terminal cwds on the desktop worker, not the executor; Park the Mac control desk ticker while no run is live; Sleep the routine scheduler until work is due instead of every 2 s; Admit terminal input from a mirror, not the lifecycle lock

### Audit cleanup (core 1/4): calmer registry saves and pushes; retired orchestrator DTOs removed

- **Completed:** 2026-09-23T08:36:30-05:00
- **Kind:** internal
- **Release:** v0.3.131
- **Commit:** `67355ae` (`cleanup/audit-core-20260923`)
- **Notes:** Remove retired orchestrator tool DTOs from contracts and TS mirror; Drop the dead iface_await long-running flag from the MCP server; Remove revoked broker bindings instead of flagging them; Delete Rust copies of TS-only contract helpers; Collapse test-only opencode worker entry points; Keep one ACP permission rule in validate_launch; Narrow engine-claude's public surface and drop dead variants; Trim engine-codex test-only helpers and unused root re-exports; Drop the unused engine_history accessor and shutdown key clones; Drop dead chrono and duplicate serde_json manifest entries; Save activity-only registry changes on a slow timer; Send timestamp-only registry pushes at most once a second; Build the Away inspect view without cloning the whole history; Clone only the last voice-control run for the display view; Read Away cycle controls and run once per Stop or feedback; Read delegate activity and terminal under one lock; End the audit peer-review hold on every exit; Load journal records without cloning each twice; Validate timestamps by shape then parse, without a round-trip; Cache Grok summaries and version across focus refreshes; Paint the cached registry before the first cold listing answers; Name the terminal arm in the registry cadence test; Remember a registry frame only once it was actually sent

### Audit cleanup (renderer 1/6): retired renderer code, dead mirrors and test-only API removed

- **Completed:** 2026-09-23T08:28:03-05:00
- **Kind:** internal
- **Release:** v0.3.131
- **Commit:** `356f2e2` (`cleanup/audit-renderer-20260923`)
- **Notes:** Drop the orphaned dompurify override from electron/package.json; Trim dead code and a redundant xattr pass from dev-app.mjs; Delete two Electron tests that exercise no production code; Delete three unreferenced shared appearance and image declarations; Delete the retired TypeScript engine adapter contract; Delete orchestration mirror types that no TypeScript reads; Drop TerminalView's unread focused prop; Delete test-only terminal helpers and fix the switcher title docs; Delete the retired terminal manager menu and its styles; Reduce ContextValueDialog to the URL form it is mounted as; Remove retired Clean GUI plumbing from the renderer; Delete no-caller renderer wrappers and a dangling AgentCanvas doc; Delete dead and test-only conversation exports, props and pill rules; Delete renderer-side agent store mutators the Rust store replaced; Delete the removed attention pill's wording and other test-only views; Drop AppSelect's always-overridden injectStyles prop and test overloads; Stop sidebarRowLists computing fields its only caller ignores; Make Clear all clear local attention events when the harness is absent; Delete SessionLibrary's no-op hover-reveal rule; Hide the attention live region with the shared sr-only rule; Drop App's preload-skew guards and a duplicate workspace ref; Keep turn checkpoints in memory instead of per-run localStorage; Delete usage-menu CSS for markup the toolbar no longer renders; Drop the narrow toolbar spacing rule that never rendered; Delete CSS rules and glyphs for markup nothing renders; Drop token restatements that change nothing in two appearance blocks

### Audit cleanup (services 1/5): dead code, dependency feature trims, blocking work off async workers

- **Completed:** 2026-09-23T08:20:57-05:00
- **Kind:** internal
- **Release:** v0.3.131
- **Commit:** `73e41be` (`cleanup/audit-services-20260923`)
- **Notes:** Install only the arm64 Rust target; Drop unused dependency features and the hex crate; Drop the async desktop dispatch wrapper and tokio dev-dependency; Delete unused desktop authority readers and restore DTO exports; Chmod iface private writes once, through the descriptor; Drop iface's never-compiled non-unix branches; Parse iface agent verbs straight into AgentMethod; Reuse ContextTelemetryCategory in iface doctor rows; Stop iface force exits from leaving an alternate screen inline; Stop iface creating an engine-history directory nothing reads; Trim interface-storage to the API its callers use; Keep the Rust invitation parser as a test oracle only; Cover the editor save-path race seams with ported oracle tests; Drop dead cfg(unix) gates in workspace, prompt-context and review; Drop ReviewCheckInvocation.shell and the ProcessOutput copy; Drop the zero-limit normalization in repository review; Drop unused serde derives from delegation ProposedFile; Drop xtask's never-compiled non-unix branches; Share one git is-ancestor helper across xtask; Stop Harness Context reading files after 20 suggestions; Skip the startup roster rewrite when no terminal name changes; Skip the reopen-authority sidecar write on title-only changes; Run prompt-context work on the blocking pool in the host; Cap iface's streaming redraws at one per 33 ms

### Update packaged smoke for the Workspaces create menu

- **Completed:** 2026-09-22T22:47:45-05:00
- **Kind:** feature
- **Release:** v0.3.130
- **Commit:** `0c0a722` (`feat/new-project-from-workspaces-20260922`)
- **Notes:** Update packaged smoke for the Workspaces create menu

### Create projects from the Workspaces plus menu

- **Completed:** 2026-09-22T21:52:23-05:00
- **Kind:** feature
- **Release:** v0.3.130
- **Commit:** `8902494` (`feat/new-project-from-workspaces-20260922`)
- **Notes:** Add project creation to the Workspaces plus menu

### Hands-free app testing for agents

- **Completed:** 2026-09-22T17:17:19-05:00
- **Kind:** feature
- **Release:** v0.3.128
- **Commit:** `00ee105` (`feat/handsfree-testing`)
- **Notes:** Hands-free UI testing for agents; Keep hands-free sessions inside the session; Sync hands-free hook: silent default-menu block

### Match reference menu transparency and charcoal tint

- **Completed:** 2026-09-22T16:51:05-05:00
- **Kind:** feature
- **Release:** v0.3.128
- **Commit:** `de42bff` (`feat/translucent-popovers-20260922`)
- **Notes:** Match the reference charcoal and visible menu translucency

### Show Claude working on turns it starts itself

- **Completed:** 2026-09-22T15:50:42-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `07f528a` (`fix/hotkey-split-blank`)
- **Notes:** Show Claude working on turns it starts itself

### Keep Codex session lists refreshing on large histories

- **Completed:** 2026-09-22T15:34:34-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `73c9b15` (`fix/hotkey-split-blank`)
- **Notes:** Keep Codex session lists refreshing on large histories

### Record pane drag globe fix

- **Completed:** 2026-09-22T15:20:54-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `312103a` (`fix/hotkey-split-blank`)
- **Notes:** Record pane drag globe fix

### No globe icon while dragging a pane

- **Completed:** 2026-09-22T15:20:40-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `ca4a86e` (`fix/hotkey-split-blank`)
- **Notes:** No globe icon while dragging a pane

### Split grid panes in place

- **Completed:** 2026-09-22T15:12:24-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `da1c88c` (`fix/hotkey-split-blank`)
- **Notes:** Split grid panes in place

### Record split hotkey follow-ups

- **Completed:** 2026-09-22T14:21:12-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `f7748dc` (`fix/hotkey-split-blank`)
- **Notes:** Record split hotkey follow-ups

### Blank terminal from the split hotkeys

- **Completed:** 2026-09-22T14:20:48-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `3a99b2a` (`fix/hotkey-split-blank`)
- **Notes:** Blank terminal from the split hotkeys

### Subtle frosted menus and dialogs

- **Completed:** 2026-09-22T10:18:05-05:00
- **Kind:** feature
- **Release:** v0.3.128
- **Commit:** `cc39ea4` (`feat/translucent-popovers-20260922`)
- **Notes:** Add subtle frosted backgrounds to floating menus and dialogs

### Record sidebar collections rollback verification

- **Completed:** 2026-09-21T21:17:35-05:00
- **Kind:** record
- **Release:** v0.3.128
- **Commit:** `fbb174c` (`revert/sidebar-collections-20260921`)
- **Notes:** Record sidebar collections rollback verification

### Restore the sidebar before the collections trial

- **Completed:** 2026-09-21T21:13:42-05:00
- **Kind:** feature
- **Release:** v0.3.128
- **Commit:** `0dacd08` (`revert/sidebar-collections-20260921`)
- **Notes:** Withdraw the sidebar collections trial

### Record workspace deletion fix staged for production

- **Completed:** 2026-09-21T20:10:54-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `cbc3520` (`fix/workspace-delete-20260921`)
- **Notes:** Record approved 0.3.127 publication and staging

### Integrate approved workspace deletion release

- **Completed:** 2026-09-21T20:08:51-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `a92d867` (`fix/workspace-delete-20260921`)
- **Notes:** Fix workspace deletion error during terminal cleanup; chore(release): bump Interface to 0.3.127

### Organize sidebar projects into collections

- **Completed:** 2026-09-21T19:47:53-05:00
- **Kind:** feature
- **Release:** v0.3.128
- **Commit:** `a34a8bd` (`feat/sidebar-collections-20260921`)
- **Notes:** Add persistent sidebar project collections

### Record workspace deletion release candidate

- **Completed:** 2026-09-21T19:30:25-05:00
- **Kind:** fix
- **Release:** v0.3.128
- **Commit:** `e8d0915` (`fix/workspace-delete-20260921`)
- **Notes:** Record verified workspace deletion fix awaiting production approval

### Fix workspace deletion error during terminal cleanup

- **Completed:** 2026-09-21T19:22:52-05:00
- **Kind:** fix
- **Release:** v0.3.127
- **Commit:** `f749dba` (`fix/workspace-delete-20260921`)
- **Notes:** Fix workspace deletion racing terminal dismissal

### Auto-tidy new terminal panes

- **Completed:** 2026-09-21T19:09:11-05:00
- **Kind:** feature
- **Release:** v0.3.127
- **Commit:** `69bc908` (`codex/auto-tidy-new-panes-20260921`)
- **Notes:** Auto-tidy the workspace when opening a new pane

### Create project folders from the app

- **Completed:** 2026-09-21T18:54:43-05:00
- **Kind:** feature
- **Release:** v0.3.127
- **Commit:** `7a0483e` (`feat/create-folders-in-picker-20260921`)
- **Notes:** Enable creating project folders from the native picker

### Record approved 0.3.126 updater staging

- **Completed:** 2026-09-21T18:52:01-05:00
- **Kind:** record
- **Release:** v0.3.127
- **Commit:** `fdd8934` (`codex/release-smoke-settled-layout-20260921`)
- **Notes:** Record approved 0.3.126 main promotion and updater staging

### Record verified 0.3.126 candidate awaiting scope approval

- **Completed:** 2026-09-21T18:49:29-05:00
- **Kind:** record
- **Release:** v0.3.127
- **Commit:** `085a41f` (`codex/release-smoke-settled-layout-20260921`)
- **Notes:** Record verified 0.3.126 candidate awaiting scope approval

### Wait for terminal resize animation in packaged smoke

- **Completed:** 2026-09-21T18:44:31-05:00
- **Kind:** feature
- **Release:** v0.3.126
- **Commit:** `8f18fa4` (`codex/release-smoke-settled-layout-20260921`)
- **Notes:** Wait for settled pane geometry in packaged smoke

### Start new panes in the workspace project, not the parent folder

- **Completed:** 2026-09-21T16:27:21-05:00
- **Kind:** fix
- **Release:** v0.3.126
- **Commit:** `b0d29fe` (`fix/grok-pane-project-directory`)
- **Notes:** Start new panes in the workspace project, not the parent folder.

### Record the Grok pane-name landing

- **Completed:** 2026-09-21T16:18:45-05:00
- **Kind:** feature
- **Release:** v0.3.126
- **Commit:** `c02f90c` (`feat/grok-pane-names`)
- **Notes:** Record the Grok pane-name landing on Dev

### Name Grok panes from the session title

- **Completed:** 2026-09-21T16:17:56-05:00
- **Kind:** feature
- **Release:** v0.3.126
- **Commit:** `0ad9db5` (`feat/grok-pane-names`)
- **Notes:** Name Grok panes from the session title

### Record verified 0.3.124 long Codex update staging

- **Completed:** 2026-09-21T15:00:41-05:00
- **Kind:** fix
- **Release:** v0.3.126
- **Commit:** `e0d5e9e` (`fix/codex-long-terminal-replay-20260919`)
- **Notes:** docs: record long Codex fix production staging and verification

### Verify retained long terminals in packaged releases

- **Completed:** 2026-09-21T14:54:01-05:00
- **Kind:** fix
- **Release:** v0.3.126
- **Commit:** `06fff32` (`fix/codex-long-terminal-replay-20260919`)
- **Notes:** test(release): verify long terminal preservation in packaged smoke

### Keep long Codex terminal fix compatible with production

- **Completed:** 2026-09-21T14:43:15-05:00
- **Kind:** fix
- **Release:** v0.3.126
- **Commit:** `8f175c3` (`fix/codex-long-terminal-replay-20260919`)
- **Notes:** Keep retained terminal reattachment independent of pane animations

### Record long Codex terminal fix verification

- **Completed:** 2026-09-19T21:41:44-05:00
- **Kind:** fix
- **Release:** v0.3.124
- **Commit:** `94e633d` (`fix/codex-long-terminal-replay-20260919`)
- **Notes:** Record verified Dev landing for long Codex terminal fix

### Preserve long Codex chats across workspace switches

- **Completed:** 2026-09-19T21:40:36-05:00
- **Kind:** fix
- **Release:** v0.3.124
- **Commit:** `92407f5` (`fix/codex-long-terminal-replay-20260919`)
- **Notes:** Preserve long-running terminal screens across workspace switches

### Sleep idle workspace terminals and resume on selection

- **Completed:** 2026-09-19T11:15:32-05:00
- **Kind:** feature
- **Release:** v0.3.124
- **Commit:** `ffa91d7` (`codex/workspace-terminal-sleep-20260919`)
- **Notes:** Suspend idle terminals in inactive workspaces

### Anthropic Light and Anthropic Dark themes

- **Completed:** 2026-09-17T22:43:40-05:00
- **Kind:** feature
- **Release:** v0.3.124
- **Commit:** `2334417` (`feat/anthropic-theme`)
- **Notes:** Record Anthropic themes in the to-do list; Extend three settled looks to the Anthropic palettes

### Fluid sidebar slide with a fixed toggle

- **Completed:** 2026-09-17T20:10:43-05:00
- **Kind:** feature
- **Release:** v0.3.124
- **Commit:** `e66b9c2` (`feat/sidebar-slide-20260917`)
- **Notes:** Fluid sidebar slide with a fixed toggle

### Record 0.3.123 release of stuck renderer self-heal

- **Completed:** 2026-09-17T12:01:41-05:00
- **Kind:** record
- **Release:** v0.3.124
- **Commit:** `c405848` (`docs/renderer-cpu-release-record-20260917`)
- **Notes:** Record 0.3.123 release of the stuck renderer self-heal

### Release copy of stuck renderer self-heal for 0.3.123

- **Completed:** 2026-09-17T11:54:15-05:00
- **Kind:** speed
- **Release:** v0.3.123
- **Commit:** `306ddec` (`perf/renderer-cpu-release-20260917`)
- **Notes:** Release-compatible self-heal for a stuck renderer compositor

### Self-heal stuck renderer CPU and pause hidden windows

- **Completed:** 2026-09-17T11:43:57-05:00
- **Kind:** speed
- **Release:** v0.3.123
- **Commit:** `22f6b98` (`perf/renderer-cpu-20260917`)
- **Notes:** Self-heal a stuck renderer compositor and truly pause hidden windows

### Prepare standalone keyboard fix release

- **Completed:** 2026-09-16T15:35:12-05:00
- **Kind:** fix
- **Release:** v0.3.121
- **Commit:** `77aec41` (`codex/release-keyboard-fix-20260916`)
- **Notes:** Fix duplicated physical keyboard typing

### Fix duplicated physical keyboard typing

- **Completed:** 2026-09-16T15:23:29-05:00
- **Kind:** fix
- **Release:** v0.3.121
- **Commit:** `6847376` (`codex/fix-duplicate-physical-typing`)
- **Notes:** Fix duplicated physical keyboard presses on macOS

### Smooth pane movement, maximize and swaps

- **Completed:** 2026-09-15T15:16:17-05:00
- **Kind:** feature
- **Release:** v0.3.121
- **Commit:** `903f5ad` (`feat/pane-motion-20260915`)
- **Notes:** feat: animate terminal pane layout changes; docs: record the approved Dev pane motion trial

### Record agent teammates landing in the to-do list

- **Completed:** 2026-09-15T13:46:40-05:00
- **Kind:** feature
- **Release:** v0.3.121
- **Commit:** `6b66ebf` (`feat/agent-teammates-core-20260915`)
- **Notes:** Record agent teammates landing on dev

### Agent teammates: home folders, threads, memory and risky-act approvals

- **Completed:** 2026-09-15T13:45:00-05:00
- **Kind:** feature
- **Release:** v0.3.121
- **Commit:** `16c183c` (`feat/agent-teammates-core-20260915`)
- **Notes:** Plan agent teammates core implementation; Agent teammates contracts, approval facts hook and protocol 23; Engine approval gate, extra directories and approval facts; Agent store v2, home folders, decision log and desktop wiring; Agent teammate thread, memory, computer tabs and roster status; Agent approval policy and quote-aware shell tokenizer; Ask the owner before agents write harness configuration; Close approval gate bypasses found in review; Agent store fixes: idle close, import grants, safe temp files, grant roots; Close agent policy gaps: exec environment, git hooks and config, credential stores; Agent page: read-only past threads, focused sessions, inline refusals, latched restart notice; Close adversarial-review policy bypasses (fix round 2); Host agent threads, approval driver and runtime guard; Agent page: read a thread only when the thread itself is open; Close same-class policy variants (fix round 3); Agent sessions read-only outside their live thread or run; Resolve git paths from the -C base directory (fix round 4); Live agent gate probe harness and agent teammates docs; Close agent gate escapes found in final review; Unwrap Codex's harness shell wrapper before classifying; Model the directory changes the agent policy missed; Read git long-option abbreviations the way git does; Disable Codex's default-on acting features for gated agents; Keep Codex's code-mode host on for gated agents; Drop the && working-directory narrowing from the agent policy; Record agent teammates live probe results and current limits

### Record application review results and verified Dev landing

- **Completed:** 2026-09-15T12:22:11-05:00
- **Kind:** speed
- **Release:** v0.3.121
- **Commit:** `13e23fa` (`perf/application-review-20260915`)
- **Notes:** Record completed application review and Dev verification

### Improve search, transcript reads and idle resource use

- **Completed:** 2026-09-15T12:19:57-05:00
- **Kind:** speed
- **Release:** v0.3.121
- **Commit:** `38d547a` (`perf/application-review-20260915`)
- **Notes:** Reduce repeated search, history, polling and replay work

### Record Away 0.3.119 staged update

- **Completed:** 2026-09-15T09:26:40-05:00
- **Kind:** record
- **Release:** v0.3.121
- **Commit:** `4c862b1` (`docs/away-promotion-handoff-20260914`)
- **Notes:** Mark Away 0.3.119 promotion and staging complete

### Design for Grok Bot-style agent teammates (spec only)

- **Completed:** 2026-09-15T08:44:14-05:00
- **Kind:** record
- **Release:** v0.3.119
- **Commit:** `2c72141` (`docs/agent-teammates-core-spec-20260915`)
- **Notes:** Design Grok Bot-style agent teammates core

### New agent asks for name, purpose and icon first

- **Completed:** 2026-09-14T21:34:47-05:00
- **Kind:** feature
- **Release:** v0.3.119
- **Commit:** `0299a14` (`feat/new-agent-identity-20260914`)
- **Notes:** New agent asks for name, purpose and icon first

### Record Grok 4.6 worker landing

- **Completed:** 2026-09-14T21:32:13-05:00
- **Kind:** feature
- **Release:** v0.3.119
- **Commit:** `bfc57a8` (`feat/grok-worker`)
- **Notes:** Record Grok 4.6 worker landing in TODO

### Grok 4.6 available in the cheap_agents worker MCP

- **Completed:** 2026-09-14T21:31:24-05:00
- **Kind:** feature
- **Release:** v0.3.119
- **Commit:** `908ec57` (`feat/grok-worker`)
- **Notes:** Admit Grok 4.6 as a cheap_agents worker model

### Record completed performance review and 0.3.118 staging

- **Completed:** 2026-09-14T21:20:45-05:00
- **Kind:** fix
- **Release:** v0.3.119
- **Commit:** `bd0606b` (`fix/review-audit-v2-20260914`)
- **Notes:** Record all review fixes staged in 0.3.118

### Record completed audit fix and prepared release approval

- **Completed:** 2026-09-14T21:17:22-05:00
- **Kind:** fix
- **Release:** v0.3.119
- **Commit:** `86bd045` (`fix/review-audit-v2-20260914`)
- **Notes:** Record audit release verification and exact publication boundary

### Preserve complete audit history with bounded append cost

- **Completed:** 2026-09-14T21:10:43-05:00
- **Kind:** fix
- **Release:** v0.3.118
- **Commit:** `f5f8c25` (`fix/review-audit-v2-20260914`)
- **Notes:** Preserve complete audit history in lossless v2 segments

### Record performance release preparation and approval boundaries

- **Completed:** 2026-09-14T20:55:51-05:00
- **Kind:** fix
- **Release:** v0.3.118
- **Commit:** `36b4792` (`fix/review-performance-20260914`)
- **Notes:** Record prepared performance release and pending approvals

### Bound parsing, history retention and worker capture

- **Completed:** 2026-09-14T20:45:47-05:00
- **Kind:** fix
- **Release:** v0.3.117
- **Commit:** `f642ef1` (`fix/review-performance-20260914`)
- **Notes:** Bound parsing, runtime content retention and worker capture; Preserve the settled header fixture beside the focus regression

### Record Away promotion scope blocker

- **Completed:** 2026-09-14T18:49:43-05:00
- **Kind:** record
- **Release:** v0.3.117
- **Commit:** `d19c511` (`docs/away-promotion-handoff-20260914`)
- **Notes:** Record blocked Away-only promotion and release scope

### Retire and archive Clean GUI

- **Completed:** 2026-09-14T17:57:42-05:00
- **Kind:** internal
- **Release:** v0.3.117
- **Commit:** `39e1cb9` (`chore/archive-clean-gui-20260914`)
- **Notes:** Retire Clean GUI and preserve its source and design archive

### One-button project Away loops

- **Completed:** 2026-09-14T15:50:58-05:00
- **Kind:** feature
- **Release:** v0.3.117
- **Commit:** `af14596` (`feat/away-one-button-20260914`)
- **Notes:** Add one-button project Away loops with native work and review

### One attachment drop cue across terminal, Clean, chat and composer

- **Completed:** 2026-09-14T14:53:50-05:00
- **Kind:** fix
- **Release:** v0.3.117
- **Commit:** `7882e96` (`fix/drop-indicator-everywhere-20260914`)
- **Notes:** One attachment drop cue across terminal, Clean, chat and composer

### Track unfinished voice work and require resumable handoffs

- **Completed:** 2026-09-14T14:50:31-05:00
- **Kind:** record
- **Release:** v0.3.117
- **Commit:** `ec02f85` (`docs/voice-control-handoff-20260914`)
- **Notes:** Record unfinished voice work and require automatic task handoffs

### Compact menus follow the BridgeMind menu reference

- **Completed:** 2026-09-14T14:47:05-05:00
- **Kind:** feature
- **Release:** v0.3.117
- **Commit:** `1308161` (`feat/pane-menu-bridgemind-20260914`)
- **Notes:** Compact menus follow the BridgeMind menu reference

### Provider icons for CLIs started from a shell pane

- **Completed:** 2026-09-14T14:38:24-05:00
- **Kind:** fix
- **Release:** v0.3.117
- **Commit:** `3608abd` (`fix/detected-provider-icon`)
- **Notes:** Show provider marks for CLIs started from a shell pane

### Clean spacing aligns messages and composer

- **Completed:** 2026-09-14T13:56:27-05:00
- **Kind:** fix
- **Release:** v0.3.117
- **Commit:** `46e743e` (`fix/clean-mode-recovery-20260914`)
- **Notes:** Align Clean messages and composer on one reading column

### Add control assistant rehearsal for desktop and iPhone

- **Completed:** 2026-09-14T12:52:22-05:00
- **Kind:** feature
- **Release:** v0.3.117
- **Commit:** `22fac77` (`feat/voice-control-20260914`)
- **Notes:** Add bounded control assistant rehearsal for desktop and iPhone

### Menus that fit no longer scroll by half a pixel

- **Completed:** 2026-09-14T12:14:18-05:00
- **Kind:** fix
- **Release:** v0.3.117
- **Commit:** `ed4ce86` (`fix/popover-fractional-height-20260914`)
- **Notes:** Menus that fit no longer scroll by half a pixel

### Clean Claude settings handle wrapped confirmations and titled prompts

- **Completed:** 2026-09-14T12:02:11-05:00
- **Kind:** fix
- **Release:** v0.3.117
- **Commit:** `8296384` (`fix/clean-mode-recovery-20260914`)
- **Notes:** Recognize wrapped Claude effort receipts under titled prompts

### Four-square button opens the workspace menu

- **Completed:** 2026-09-14T11:54:01-05:00
- **Kind:** feature
- **Release:** v0.3.117
- **Commit:** `3425e89` (`feat/grid-menu-20260914`)
- **Notes:** Four-square button opens the workspace menu

### Clean settings recover across Ultracode, redraws and request failures

- **Completed:** 2026-09-14T11:49:54-05:00
- **Kind:** fix
- **Release:** v0.3.117
- **Commit:** `3755b2c` (`fix/clean-mode-recovery-20260914`)
- **Notes:** Fix Clean settings confirmation, uncertainty and recovery; Fence Clean settings while native yes/no approvals own input

### Faster promote: optimised hashing, reused renderer build and icons

- **Completed:** 2026-09-14T11:04:27-05:00
- **Kind:** speed
- **Release:** v0.3.117
- **Commit:** `b87f6a8` (`perf/promote-redundant-steps-20260914`)
- **Notes:** Faster promote: optimised hashing, reused renderer build and icons

### Settings page follows the BridgeMind settings reference

- **Completed:** 2026-09-14T11:01:20-05:00
- **Kind:** feature
- **Release:** v0.3.117
- **Commit:** `6a1cd64` (`feat/settings-reference-20260914`)
- **Notes:** Keep the Pane appearance row line untouched

### Repair Claude Clean detection and remove duplicate effort control

- **Completed:** 2026-09-14T10:36:47-05:00
- **Kind:** fix
- **Release:** v0.3.115
- **Commit:** `ab4ef88` (`fix/clean-mode-recovery-20260914`)
- **Notes:** Recognize native Claude in Clean and keep one settings menu

### OpenCode typed into a shell pane repaints after switching back

- **Completed:** 2026-09-14T10:31:05-05:00
- **Kind:** fix
- **Release:** v0.3.115
- **Commit:** `efc6e13` (`fix/shell-tui-replay-20260914`)
- **Notes:** OpenCode typed into a shell pane repaints after switching back

### Pane + opens the New pane menu; file drops show a path pill

- **Completed:** 2026-09-14T10:25:15-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `1f151cc` (`feat/pane-menu-drop-20260914`)
- **Notes:** Pane + opens the New pane menu; file drops show a path pill

### New panes share their row equally

- **Completed:** 2026-09-14T10:11:45-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `676a139` (`feat/pane-row-insert-20260914`)
- **Notes:** New panes share their row equally

### Restore reliable Clean model and effort controls

- **Completed:** 2026-09-14T10:00:24-05:00
- **Kind:** fix
- **Release:** v0.3.115
- **Commit:** `2783c49` (`fix/clean-mode-recovery-20260914`)
- **Notes:** Keep Clean settings changes working through terminal focus

### Repair Clean layout and restore working controls

- **Completed:** 2026-09-14T09:38:37-05:00
- **Kind:** fix
- **Release:** v0.3.115
- **Commit:** `0d626e3` (`fix/clean-mode-recovery-20260914`)
- **Notes:** Repair Clean layout and restore working controls

### Clean/terminal: retry saved-pane reopen through the transient host-restart window

- **Completed:** 2026-09-14T08:40:11-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `571ed22` (`feat/clean-fresh-direction-b-20260914`)
- **Notes:** Clean/terminal: retry saved-pane reopen through the transient restart window; docs(renderer): note the bounded reopen retry for saved panes

### Cheap Agents MCP name for Claude and Codex

- **Completed:** 2026-09-14T08:22:19-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `c513975` (`feat/cheap-agents-name-20260914`)
- **Notes:** Name the worker MCP cheap_agents for Claude and Codex

### Clean: fresh Claude/Codex panes open on Direction B instead of the raw-CLI reader

- **Completed:** 2026-09-14T07:57:35-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `d00df76` (`feat/clean-fresh-direction-b-20260914`)
- **Notes:** Clean: fresh Claude/Codex panes open on Direction B, not the raw CLI; docs(renderer): note the provisional Clean source for fresh panes

### Reliable cheap workers in Claude and Codex panes

- **Completed:** 2026-09-13T22:47:33-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `e651d84` (`feat/worker-mcp-20260913`)
- **Notes:** Make report workers reliable across native Claude and Codex panes; Fence older worker brokers before archived history is admitted

### Clean Direction B: approval card answers CLI prompts, in-pane Review changes with quote to prompt

- **Completed:** 2026-09-13T22:32:12-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `c9e5d8f` (`feat/clean-direction-b-20260913`)
- **Notes:** Clean approval card: answer CLI prompts in place; Clean review sheet: in-pane diff with quote to prompt; Clean approval card: the list the CLI printed, the key it printed; Clean approval card: the shortcut the dialog prints, and a mark that lifts; Renderer notes for the approval card and review sheet; Clean review sheet: the turn you pressed, the keys, the small pane

### Shared cheap report workers for Claude and Codex

- **Completed:** 2026-09-13T21:55:57-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `b77b22c` (`feat/worker-mcp-20260913`)
- **Notes:** Add opt-in shared report workers MCP for Claude and Codex

### OpenCode sessions start with auto permissions

- **Completed:** 2026-09-13T21:10:35-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `40020dc` (`feat/opencode-auto-permissions-20260913`)
- **Notes:** Launch OpenCode panes with auto permissions

### Clean Direction B: panels, status pill, shimmer indicator, composer card, model menu, reveal strip

- **Completed:** 2026-09-13T21:00:49-05:00
- **Kind:** feature
- **Release:** v0.3.115
- **Commit:** `10c084f` (`feat/clean-direction-b-20260913`)
- **Notes:** Clean pane Direction B spec and plan; Clean composer card: one row until focused, model menu, Stop and Queue, reveal strip; Clean transcript: token palette, work panels, status pill, shimmer indicator; Clean composer review fixes: real copy, a name in the pill, slash returns itself; Clean review fixes: sink alignment, head order, declined edits; Clean Direction B: settled look and design notes; Hide the Clean effort pill below 430px, and keep the reveal rail on one line

### OpenCode panes recover after busy workspace switches

- **Completed:** 2026-09-13T19:37:57-05:00
- **Kind:** fix
- **Release:** v0.3.114
- **Commit:** `2a2d94d` (`fix/opencode-replay-20260913`)
- **Notes:** Wait for OpenCode to process terminal redraw resizes

### Pane Expand and New tab always visible

- **Completed:** 2026-09-13T19:29:27-05:00
- **Kind:** fix
- **Release:** v0.3.114
- **Commit:** `f8aa49f` (`fix/pane-actions-always-visible`)
- **Notes:** Keep pane Expand and New tab visible at rest

### Clean review: approvals reveal, queued sends, mentions, reading polish, input setting

- **Completed:** 2026-09-13T17:13:08-05:00
- **Kind:** feature
- **Release:** v0.3.114
- **Commit:** `3fd69e6` (`feat/clean-review-20260913`)
- **Notes:** Clean input setting: composer or native CLI prompt; Clean reading polish: named folds, result phrase, neutral composer, strip and short-pane fixes; Name a lone Clean fold with its verb, and leave edits their diffstat; Clean composer: reveal approvals, hand off slash commands, queue while working; Reveal the CLI only when it is holding the keyboard; Veil the rail from the rail, and never say a fold's verb twice; Hold the Clean presentation while the emulator is scrolled back; Paint Clean's reading veil inside the scroller, under anything pinned; Clean composer @-mentions and Open changes on the turn chip; Clean review plan and renderer notes; Clean review fixes: findable payloads, honest folds and a pressable chip

### Confirm extended-context models and finish Clean composer

- **Completed:** 2026-09-13T14:24:54-05:00
- **Kind:** feature
- **Release:** v0.3.114
- **Commit:** `60f3354` (`feat/clean-composer-20260913`)
- **Notes:** Confirm extended-context models and quiet routine draft status

### Custom Clean composer with model and effort selectors

- **Completed:** 2026-09-13T14:10:01-05:00
- **Kind:** feature
- **Release:** v0.3.114
- **Commit:** `0fd1297` (`feat/clean-composer-20260913`)
- **Notes:** Use a custom Clean composer with model and effort selectors

### Color and depth for Clean conversations

- **Completed:** 2026-09-13T13:43:07-05:00
- **Kind:** feature
- **Release:** v0.3.114
- **Commit:** `d6dc6f4` (`feat/clean-color-20260913`)
- **Notes:** Add color and depth to Clean conversations

### Cleaner reading inspired by T3 Code and MonoCode

- **Completed:** 2026-09-13T12:44:22-05:00
- **Kind:** feature
- **Release:** v0.3.114
- **Commit:** `3c87e9f` (`feat/clean-reading-20260913`)
- **Notes:** Simplify Clean reading with quieter chrome and folded activity

### Clean pane guidance and stable disclosure verification

- **Completed:** 2026-09-13T12:15:05-05:00
- **Kind:** feature
- **Release:** v0.3.114
- **Commit:** `85a151d` (`feat/clean-native-panes-20260913`)
- **Notes:** Clarify Clean native input and await rendered tool details

### Clean conversations with native CLI input

- **Completed:** 2026-09-13T12:04:58-05:00
- **Kind:** feature
- **Release:** v0.3.114
- **Commit:** `d37bcdc` (`feat/clean-native-panes-20260913`)
- **Notes:** Present clean conversations above native CLI input

### Harness commands, task evidence, shared drafts and bounded pairing

- **Completed:** 2026-09-12T22:23:08-05:00
- **Kind:** feature
- **Release:** v0.3.113
- **Commit:** `a9df308` (`feat/harness-usability-20260912`)
- **Notes:** Add harness commands, durable task evidence and shared drafts

### Packaged smoke pins the four-column launcher roster

- **Completed:** 2026-09-12T14:19:09-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `afe6471` (`feat/aesthetic-review-20260912`)
- **Notes:** Packaged smoke pins the launcher's four-column roster

### Text cursor over selectable terminal text

- **Completed:** 2026-09-12T13:47:54-05:00
- **Kind:** fix
- **Release:** v0.3.112
- **Commit:** `e05c485` (`fix/terminal-text-cursor-20260912`)
- **Notes:** Text cursor over selectable terminal text

### Quiet New-tab trigger on pane heads

- **Completed:** 2026-09-12T13:39:36-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `54277c1` (`feat/aesthetic-review-20260912`)
- **Notes:** Quiet the New-tab menu trigger too

### Aesthetic review: quieter chrome, one label idiom, one Send, product mark

- **Completed:** 2026-09-12T13:34:34-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `c5f6470` (`feat/aesthetic-review-20260912`)
- **Notes:** Reveal quiet pane actions through :is(), leaving the hover-tint marker untouched

### Efficiency cleanup: push batching, uuid helper, orchestrator retirement, App.tsx hooks

- **Completed:** 2026-09-12T13:26:11-05:00
- **Kind:** internal
- **Release:** v0.3.112
- **Commit:** `eae231e` (`cleanup/efficiency-20260912`)
- **Notes:** Ignore iCloud conflict copies and ship an optimised release profile; One canonical UUID check in interface-common; Flush queued host pushes together; Retire the unimplemented orchestrator host from the collaboration broker; Lift six self-contained state groups out of App.tsx into hooks

### Pane-close visibility guard in the collapse fixture

- **Completed:** 2026-09-12T12:53:44-05:00
- **Kind:** fix
- **Release:** v0.3.112
- **Commit:** `3fd566d` (`fix/pane-close-slot-check`)
- **Notes:** Assert survivors stay visible after each pane close

### Run reviewed Away cycles from Codex and Claude Code

- **Completed:** 2026-09-12T12:32:40-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `9f4e2e6` (`feat/away-command-review-20260912`)
- **Notes:** Add reviewed Away cycles for Codex and Claude Code

### Reject malformed Away baseline evidence

- **Completed:** 2026-09-12T11:44:49-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `267e430` (`feat/away-validation-evidence-20260912`)
- **Notes:** Reject malformed Away baseline evidence

### Record Away baseline evidence landing

- **Completed:** 2026-09-12T11:41:37-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `137538e` (`feat/away-validation-evidence-20260912`)
- **Notes:** Record Away baseline evidence landing

### Retain failed Away baseline evidence

- **Completed:** 2026-09-12T11:40:33-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `305255a` (`feat/away-validation-evidence-20260912`)
- **Notes:** Retain failed Away baseline evidence

### Record reviewed Away correction landing

- **Completed:** 2026-09-12T11:37:08-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `9a83cc7` (`feat/away-validation-evidence-20260912`)
- **Notes:** Record reviewed Away correction landing

### Compare Away candidate changes for no progress

- **Completed:** 2026-09-12T11:36:07-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `98f65e8` (`feat/away-validation-evidence-20260912`)
- **Notes:** Compare Away candidate changes for no progress

### Close Away cycle at deadline

- **Completed:** 2026-09-12T11:29:57-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `ca8b908` (`feat/away-validation-evidence-20260912`)
- **Notes:** Close Away cycle at deadline

### Record Away vault handoff blocker

- **Completed:** 2026-09-12T11:15:18-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `cdea1cc` (`feat/away-validation-evidence-20260912`)
- **Notes:** Record Away vault handoff blocker

### Record Away cycle audit and runtime evidence

- **Completed:** 2026-09-12T11:11:47-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `7498eea` (`feat/away-validation-evidence-20260912`)
- **Notes:** Record Away cycle audit and runtime evidence

### Retain Away validation infrastructure failures

- **Completed:** 2026-09-12T11:09:58-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `defdd49` (`feat/away-validation-evidence-20260912`)
- **Notes:** Retain Away validation infrastructure failures

### Retain Away baseline evidence before dispatch

- **Completed:** 2026-09-12T11:05:16-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `d5ca20c` (`feat/away-baseline-20260912`)
- **Notes:** Retain Away baseline evidence before dispatch

### Expose unknown Away outcomes to status callers

- **Completed:** 2026-09-12T11:00:33-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `234da0d` (`feat/away-status-20260912`)
- **Notes:** Expose unknown Away outcomes to status callers

### Honor Away Stop during provider waits

- **Completed:** 2026-09-12T10:54:01-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `675d2aa` (`feat/away-stop-20260912`)
- **Notes:** Honor Away Stop during provider waits

### Retain Away validation context across retries

- **Completed:** 2026-09-12T10:48:10-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `fcd46e5` (`feat/away-feedback-20260912`)
- **Notes:** Retain Away validation context across retries

### Prevent Away from spending retries on duplicate failed proposals

- **Completed:** 2026-09-12T10:41:38-05:00
- **Kind:** feature
- **Release:** v0.3.112
- **Commit:** `d2250d0` (`feat/away-hour-20260912`)
- **Notes:** Stop Away retries after repeated proposals

### Pair iPhone when local discovery is unavailable

- **Completed:** 2026-09-12T10:13:04-05:00
- **Kind:** fix
- **Release:** v0.3.112
- **Commit:** `88c9c5a` (`fix/phone-direct-address-20260912`)
- **Notes:** Fix iPhone pairing when Bonjour discovery is unavailable

### Checked agent navigation and bounded-task guide

- **Completed:** 2026-09-12T10:01:25-05:00
- **Kind:** record
- **Release:** v0.3.112
- **Commit:** `986f891` (`docs/agent-navigation-20260912`)
- **Notes:** Add checked task navigation and agent audit; Record navigation audit validation evidence

### Click to place the cursor in unshaded Codex prompts

- **Completed:** 2026-09-11T23:06:38-05:00
- **Kind:** fix
- **Release:** v0.3.109
- **Commit:** `78a73dd` (`fix/terminal-click-cursor-20260911`)
- **Notes:** Recognize the unshaded native Codex prompt layout

### Bounded Away pilot with validated repairs and Stop

- **Completed:** 2026-09-11T23:03:23-05:00
- **Kind:** feature
- **Release:** v0.3.109
- **Commit:** `1046a3d` (`feat/away-pilot-20260911`)
- **Notes:** Add bounded Away pilot with durable validation and cancellation

### Pin projects alongside workspaces

- **Completed:** 2026-09-11T22:59:47-05:00
- **Kind:** feature
- **Release:** v0.3.109
- **Commit:** `2c2f92b` (`feat/project-pinning-20260911`)
- **Notes:** Add persistent project pins alongside workspace pins

### Fix black panes when opening or switching workspaces

- **Completed:** 2026-09-11T22:56:29-05:00
- **Kind:** fix
- **Release:** v0.3.109
- **Commit:** `49f802e` (`fix/blank-new-panes-20260911`)
- **Notes:** Remove retired pane mounts when switching workspaces

### Click to place the cursor in terminal prompts

- **Completed:** 2026-09-11T22:52:46-05:00
- **Kind:** fix
- **Release:** v0.3.109
- **Commit:** `8137568` (`fix/terminal-click-cursor-20260911`)
- **Notes:** Add click-to-place cursor in native terminal prompts

### Encrypted iPhone QR pairing and direct connection

- **Completed:** 2026-09-11T21:00:29-05:00
- **Kind:** feature
- **Release:** v0.3.107
- **Commit:** `aa46d6f` (`feat/iphone-direct-connect-20260911`)
- **Notes:** feat(remote): add encrypted QR pairing and native phone transport

### Redesign iPhone sessions, conversations and approvals

- **Completed:** 2026-09-11T18:25:06-05:00
- **Kind:** feature
- **Release:** v0.3.106
- **Commit:** `3564df8` (`feat/iphone-design-20260911`)
- **Notes:** Redesign the iPhone library and conversation experience

### Stop iPhone Face ID retry loops and recover Mac pairing

- **Completed:** 2026-09-11T17:59:55-05:00
- **Kind:** fix
- **Release:** v0.3.106
- **Commit:** `00645e7` (`fix/iphone-faceid-loop-20260911`)
- **Notes:** Prevent repeated iPhone Face ID prompts after refused pairing

### Interface iPhone app icon and production connection guide

- **Completed:** 2026-09-11T17:25:55-05:00
- **Kind:** feature
- **Release:** v0.3.105
- **Commit:** `e8da966` (`feat/iphone-brand-production-20260911`)
- **Notes:** Add Interface Fold branding to the iPhone app

### Refined iPhone conversations with replies and approvals

- **Completed:** 2026-09-11T17:04:27-05:00
- **Kind:** feature
- **Release:** v0.3.105
- **Commit:** `963bb47` (`feat/iphone-conversation-20260911`)
- **Notes:** Refined iPhone conversations with replies and single-use approvals

### Prevent terminal animation freezes

- **Completed:** 2026-09-11T16:02:26-05:00
- **Kind:** fix
- **Release:** v0.3.104
- **Commit:** `c4d01b7` (`fix/terminal-announcement-lag-20260911`)
- **Notes:** Bound terminal announcements to prevent animation layout freezes

### Trusted iPhone pairing with Face ID and native workspaces

- **Completed:** 2026-09-11T14:53:12-05:00
- **Kind:** feature
- **Release:** v0.3.104
- **Commit:** `af32d1f` (`feat/iphone-trusted-access-20260911`)
- **Notes:** Add trusted iPhone pairing and native Face ID workspace navigation

### Native iPhone companion with shared Interface sessions

- **Completed:** 2026-09-11T12:45:35-05:00
- **Kind:** feature
- **Release:** v0.3.104
- **Commit:** `17607a1` (`feat/iphone-app-20260911`)
- **Notes:** Add native iPhone companion with shared workspace and conversation views; Handle unavailable Mac gateways in the iPhone shell

### Show a floating Swap preview when dragging terminal panes

- **Completed:** 2026-09-11T12:36:39-05:00
- **Kind:** feature
- **Release:** v0.3.104
- **Commit:** `53468f3` (`feat/pane-swap-preview-20260911`)
- **Notes:** Add a floating terminal pane swap preview

### Stable Codex pane and workspace names

- **Completed:** 2026-09-11T09:40:40-05:00
- **Kind:** fix
- **Release:** v0.3.103
- **Commit:** `b2f3690` (`fix/stable-codex-pane-names-20260911`)
- **Notes:** Keep Codex pane and workspace task names stable

### iPhone remote companion for terminal sessions

- **Completed:** 2026-09-11T09:36:07-05:00
- **Kind:** feature
- **Release:** v0.3.103
- **Commit:** `eeedf7b` (`feat/iphone-remote-20260911`)
- **Notes:** Add iPhone remote companion for native terminal sessions

### Clarify terminal interaction validation

- **Completed:** 2026-09-11T08:40:03-05:00
- **Kind:** speed
- **Release:** v0.3.103
- **Commit:** `0368903` (`perf/terminal-interaction-20260911`)
- **Notes:** Clarify the interaction follow-up scoped validation

### Lighter terminal focus and pane switching

- **Completed:** 2026-09-11T08:37:33-05:00
- **Kind:** speed
- **Release:** v0.3.103
- **Commit:** `7f1dbe6` (`perf/terminal-interaction-20260911`)
- **Notes:** Avoid redundant terminal focus renders and geometry reads

### Faster terminal output and lighter idle work

- **Completed:** 2026-09-11T00:22:22-05:00
- **Kind:** speed
- **Release:** v0.3.102
- **Commit:** `77cb108` (`perf/responsiveness-20260910`)
- **Notes:** Avoid quadratic terminal scans and redundant idle updates

### Restore full-width terminal input spacing

- **Completed:** 2026-09-10T23:49:39-05:00
- **Kind:** fix
- **Release:** v0.3.102
- **Commit:** `af80b28` (`fix/restore-terminal-edge-spacing-20260910`)
- **Notes:** Restore full-width native terminal input spacing

### Align packaged smoke with compact workspace layout

- **Completed:** 2026-09-10T17:14:10-05:00
- **Kind:** fix
- **Release:** v0.3.101
- **Commit:** `da15c36` (`fix/compact-layout-package-smoke-20260910`)
- **Notes:** Align packaged smoke with compact workspace layout

### Restore compact Interface scale with reference layout and colors

- **Completed:** 2026-09-10T16:54:52-05:00
- **Kind:** fix
- **Release:** v0.3.101
- **Commit:** `d941f35` (`fix/compact-reference-layout-20260910`)
- **Notes:** Restore compact Interface scale with reference layout and colors

### Reduce terminal text to 18px

- **Completed:** 2026-09-10T16:41:57-05:00
- **Kind:** fix
- **Release:** v0.3.101
- **Commit:** `0bf3e72` (`fix/terminal-reading-scale-18px-20260910`)
- **Notes:** Reduce default terminal reading size to 18px

### Match BridgeMind workspace scale and navigation

- **Completed:** 2026-09-10T16:11:14-05:00
- **Kind:** feature
- **Release:** v0.3.101
- **Commit:** `baf4b65` (`feat/reference-workspace-scale-20260910`)
- **Notes:** Match the reference workspace scale and navigation

### Align packaged smoke checks with the approved workspace layout

- **Completed:** 2026-09-10T14:56:59-05:00
- **Kind:** fix
- **Release:** v0.3.99
- **Commit:** `8b407c6` (`fix/release-layout-smoke-20260910`)
- **Notes:** Align packaged smoke checks with the approved workspace layout

### Workspace launcher with provider tiles and launch preview

- **Completed:** 2026-09-10T11:04:39-05:00
- **Kind:** feature
- **Release:** v0.3.99
- **Commit:** `ceddfaa` (`feat/workspace-launcher-reference-20260910`)
- **Notes:** Match workspace launcher to the provider-grid reference

### Make terminal scrollbars thin and subdued

- **Completed:** 2026-09-10T10:52:51-05:00
- **Kind:** fix
- **Release:** v0.3.99
- **Commit:** `68f6930` (`fix/thin-terminal-scrollbars-20260910`)
- **Notes:** Make terminal scrollbars thin and subdued

### Prepare the standalone pane header effort removal

- **Completed:** 2026-09-10T10:23:03-05:00
- **Kind:** fix
- **Release:** v0.3.97
- **Commit:** `b1f7830` (`fix/production-header-effort-20260910`)
- **Notes:** Remove the effort selector from terminal pane headers

### Validate terminal padding through late CLI resize redraws

- **Completed:** 2026-09-10T08:49:37-05:00
- **Kind:** feature
- **Release:** v0.3.96
- **Commit:** `b58ea4d` (`release/terminal-side-padding-20260910`)
- **Notes:** Remove native terminal side padding

### Keep terminal spacing current when reopening Dev

- **Completed:** 2026-09-10T08:24:13-05:00
- **Kind:** fix
- **Release:** v0.3.96
- **Commit:** `923f81b` (`fix/dev-dock-terminal-spacing-20260910`)
- **Notes:** Refresh the built renderer before launching Dev

### Expand native terminal entry bars to the pane edges

- **Completed:** 2026-09-09T23:10:34-05:00
- **Kind:** fix
- **Release:** v0.3.96
- **Commit:** `55d6645` (`fix/terminal-edge-spacing-20260909`)
- **Notes:** Expand native terminal content to the pane edges; Record native Clean controls using the full pane width

### Give workspace names and reading controls more room

- **Completed:** 2026-09-09T23:03:38-05:00
- **Kind:** feature
- **Release:** v0.3.96
- **Commit:** `6ca89da` (`feat/readable-workspace-20260909`)
- **Notes:** Give workspace names and reading controls more room

### Remove effort selector from pane headers

- **Completed:** 2026-09-09T21:02:21-05:00
- **Kind:** fix
- **Release:** v0.3.96
- **Commit:** `cef5eca` (`fix/remove-header-effort-20260909`)
- **Notes:** Remove effort selector from pane headers

### Readable workspace hierarchy and task titles

- **Completed:** 2026-09-09T20:20:25-05:00
- **Kind:** feature
- **Release:** v0.3.96
- **Commit:** `2edcd2b` (`feat/workspace-hierarchy-20260909`)
- **Notes:** Refine workspace hierarchy, readability, and automatic task titles

### Compact header controls and readable notification counts

- **Completed:** 2026-09-09T18:41:25-05:00
- **Kind:** fix
- **Release:** v0.3.94
- **Commit:** `32e0ed6` (`fix/compact-workspace-header-20260909`)
- **Notes:** Refine workspace header proportions and notification count

### Native pane tools retain access after workspace reopen

- **Completed:** 2026-09-09T17:04:08-05:00
- **Kind:** feature
- **Release:** v0.3.93
- **Commit:** `07341f8` (`feat/pane-menu-actions-20260909`)
- **Notes:** Retain native pane tool access after saved workspace reopen

### Canvas follows folders opened by terminal panes

- **Completed:** 2026-09-09T16:53:08-05:00
- **Kind:** feature
- **Release:** v0.3.93
- **Commit:** `28dc662` (`feat/pane-menu-actions-20260909`)
- **Notes:** Keep terminal-only project folders in the Canvas picker

### Workspace tools and existing agent launches in the pane menu

- **Completed:** 2026-09-09T16:47:48-05:00
- **Kind:** feature
- **Release:** v0.3.93
- **Commit:** `2937846` (`feat/pane-menu-actions-20260909`)
- **Notes:** Add workspace tools and existing launchers to the pane menu

### Settings below Workspaces

- **Completed:** 2026-09-09T16:10:32-05:00
- **Kind:** fix
- **Release:** v0.3.92
- **Commit:** `0fed591` (`fix/sidebar-settings-20260909`)
- **Notes:** Move Settings below Workspaces in the sidebar; Record the approved sidebar Settings footer

### Effort selector matches pane header

- **Completed:** 2026-09-09T15:08:19-05:00
- **Kind:** fix
- **Release:** v0.3.91
- **Commit:** `ba895e2` (`fix/effort-header-style-20260909`)
- **Notes:** Match effort selector to pane header controls

### Compact centered Agent Code Chat header

- **Completed:** 2026-09-09T11:55:36-05:00
- **Kind:** fix
- **Release:** v0.3.90
- **Commit:** `e961a7b` (`fix/topbar-reference-20260909`)
- **Notes:** Match the window header to the compact navigation reference

### Visible idle pane borders and tighter bottom spacing

- **Completed:** 2026-09-09T10:29:44-05:00
- **Kind:** fix
- **Release:** v0.3.89
- **Commit:** `393d9d9` (`fix/pane-edges-footer-20260909`)
- **Notes:** Keep idle pane borders visible and tighten bottom spacing

### Faster transcripts and bounded background work

- **Completed:** 2026-09-09T09:36:55-05:00
- **Kind:** speed
- **Release:** v0.3.88
- **Commit:** `5820280` (`perf/review-fixes-20260909`)
- **Notes:** perf: bound presentation work and simplify controller ownership

### Durable agents and local scheduled routines

- **Completed:** 2026-09-09T09:04:41-05:00
- **Kind:** feature
- **Release:** v0.3.88
- **Commit:** `f7b21fe` (`feat/agent-foundation-20260909`)
- **Notes:** Build durable agents and local scheduled routines

### Two-by-three workspace activity ticker

- **Completed:** 2026-09-09T08:39:05-05:00
- **Kind:** fix
- **Release:** v0.3.88
- **Commit:** `b3b3b6d` (`fix/sidebar-ticker-2x3-20260909`)
- **Notes:** fix(sidebar): use a two-by-three workspace ticker

### Sidebar reference styling and active workspace ticker

- **Completed:** 2026-09-09T08:30:42-05:00
- **Kind:** feature
- **Release:** v0.3.88
- **Commit:** `a15bb2a` (`feat/sidebar-reference-20260909`)
- **Notes:** Match the sidebar reference and tick active CLI workspaces

### Keep top-bar effort menus open when changing panes

- **Completed:** 2026-09-09T08:25:53-05:00
- **Kind:** feature
- **Release:** v0.3.87
- **Commit:** `27f9212` (`feat/topbar-effort-20260909`)
- **Notes:** Keep effort menus focused when activating another pane

### Agent effort selector in terminal pane top bars

- **Completed:** 2026-09-09T08:11:45-05:00
- **Kind:** feature
- **Release:** v0.3.87
- **Commit:** `1aba3c5` (`feat/topbar-effort-20260909`)
- **Notes:** Add agent effort selection to terminal pane headers

### Reserve the Code sidebar for CLI workspaces

- **Completed:** 2026-09-09T08:05:44-05:00
- **Kind:** fix
- **Release:** v0.3.87
- **Commit:** `8f0eaf9` (`fix/cli-only-code-20260909`)
- **Notes:** Keep non-CLI conversations out of the Code sidebar

### Correct Chat and Code conversation categories

- **Completed:** 2026-09-09T07:44:40-05:00
- **Kind:** fix
- **Release:** v0.3.87
- **Commit:** `738e7ab` (`fix/chat-code-categories-20260909`)
- **Notes:** Keep uncategorized conversations under Chat

### Disable the production delegation layer

- **Completed:** 2026-09-09T07:31:47-05:00
- **Kind:** fix
- **Release:** v0.3.86
- **Commit:** `5c060e6` (`fix/disable-production-delegation-20260909`)
- **Notes:** Disable the production delegation workflow while retaining its implementation

### Grey selected pane headers; purple on hover

- **Completed:** 2026-09-08T23:20:54-05:00
- **Kind:** fix
- **Release:** v0.3.86
- **Commit:** `f9314d1` (`fix/selected-pane-grey-20260908`)
- **Notes:** Restore grey selected pane headers

### Restore dots in the sidebar and pane headers

- **Completed:** 2026-09-08T23:14:53-05:00
- **Kind:** fix
- **Release:** v0.3.86
- **Commit:** `0775ae3` (`fix/clean-sidebar-dot-20260909`)
- **Notes:** Restore sidebar and pane-header working dots

### Model and effort controls in Clean terminal bottom bars

- **Completed:** 2026-09-08T20:50:21-05:00
- **Kind:** feature
- **Release:** v0.3.85
- **Commit:** `65769be` (`feat/clean-bottom-controls-20260908`)
- **Notes:** Add model and effort controls to Clean terminal composers; Preserve settings and attachment errors in Clean composers

### Wave working indicators throughout Clean mode

- **Completed:** 2026-09-08T20:39:49-05:00
- **Kind:** feature
- **Release:** v0.3.85
- **Commit:** `eeefdca` (`feat/clean-working-wave-20260909`)
- **Notes:** Use the Wave working indicator throughout Clean mode; Record the approved Wave in Clean appearance decisions

### Close finished delegated worker panes automatically

- **Completed:** 2026-09-08T20:24:45-05:00
- **Kind:** fix
- **Release:** v0.3.85
- **Commit:** `a7eb05b` (`fix/delegate-pane-auto-close-20260908`)
- **Notes:** Close delegated worker panes when their work finishes

### Image attachments in Clean terminal conversations

- **Completed:** 2026-09-08T20:18:05-05:00
- **Kind:** fix
- **Release:** v0.3.85
- **Commit:** `3e7f946` (`fix/clean-image-attachments-20260908`)
- **Notes:** Fix image attachments in Clean terminal conversations

### Visible Clean composer actions in dark themes

- **Completed:** 2026-09-08T19:30:35-05:00
- **Kind:** fix
- **Release:** v0.3.84
- **Commit:** `4061cde` (`fix/clean-conversation-panes-20260908`)
- **Notes:** Fix Clean composer action contrast in dark themes

### Clean terminal conversations with chat input

- **Completed:** 2026-09-08T19:21:42-05:00
- **Kind:** fix
- **Release:** v0.3.84
- **Commit:** `70d3dc6` (`fix/clean-conversation-panes-20260908`)
- **Notes:** Render Clean terminal panes as conversations

### Swap pane positions when dragging their headers

- **Completed:** 2026-09-08T18:53:25-05:00
- **Kind:** fix
- **Release:** v0.3.84
- **Commit:** `60f7544` (`fix/pane-header-swap-20260908`)
- **Notes:** Swap whole terminal panes on header drop

### Show native OpenCode workers and prefer GLM or DeepSeek

- **Completed:** 2026-09-08T18:17:25-05:00
- **Kind:** fix
- **Release:** v0.3.83
- **Commit:** `dcfc575` (`fix/delegate-native-session-20260908`)
- **Notes:** Show native OpenCode workers and prefer GLM or DeepSeek

### Read CLI output in Clean panes

- **Completed:** 2026-09-08T18:07:31-05:00
- **Kind:** feature
- **Release:** v0.3.83
- **Commit:** `e6ffdb2` (`feat/clean-cli-reader-20260908`)
- **Notes:** Render CLI output as readable Clean panes

### Recover from blocked delegation handoffs

- **Completed:** 2026-09-08T17:42:58-05:00
- **Kind:** fix
- **Release:** v0.3.81
- **Commit:** `2a6b89d` (`fix/delegation-handoff-recovery-20260908`)
- **Notes:** Fix recovery after a blocked delegation handoff

### Purple header hover without pane outline

- **Completed:** 2026-09-08T15:40:46-05:00
- **Kind:** fix
- **Release:** v0.3.80
- **Commit:** `9433310` (`fix/pane-header-purple-hover-20260908`)
- **Notes:** Limit pane hover to a purple header tint

### Pane header hover glow

- **Completed:** 2026-09-08T15:07:45-05:00
- **Kind:** feature
- **Release:** v0.3.79
- **Commit:** `da212f0` (`feat/pane-header-hover-20260908`)
- **Notes:** Add soft pane header hover highlight

### Check the folder-scoped Agent composer in packaged smoke

- **Completed:** 2026-09-08T15:04:49-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `1b0f47f` (`feat/unified-pane-appearance-20260908`)
- **Notes:** Check folder-scoped Agent composer in packaged smoke

### Delegate between Claude and Codex alongside OpenCode and Antigravity

- **Completed:** 2026-09-08T14:59:52-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `bd4eb77` (`feat/claude-codex-delegation-20260908`)
- **Notes:** Support Claude and Codex as bounded delegation workers

### Restore terminal launches in saved workspaces after restart

- **Completed:** 2026-09-08T14:56:35-05:00
- **Kind:** fix
- **Release:** v0.3.78
- **Commit:** `563f980` (`fix/terminal-approved-root-20260908`)
- **Notes:** Fix fresh terminal launches in saved workspace roots after restart

### Check Chat composer independently of Code startup

- **Completed:** 2026-09-08T12:46:19-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `7dd2040` (`feat/unified-pane-appearance-20260908`)
- **Notes:** Open Chat explicitly for packaged composer checks

### Verify unified workspace creation in packaged smoke

- **Completed:** 2026-09-08T12:38:15-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `7c60ad1` (`feat/unified-pane-appearance-20260908`)
- **Notes:** Keep packaged workspace diagnostics aligned with unified creation

### Open Code at its CLI workspace on startup

- **Completed:** 2026-09-08T12:25:27-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `d0b402e` (`feat/unified-pane-appearance-20260908`)
- **Notes:** Open Code at its CLI workspace on startup

### Use one CLI workspace with Terminal or Clean appearance

- **Completed:** 2026-09-08T12:16:57-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `1d928ee` (`feat/unified-pane-appearance-20260908`)
- **Notes:** Make Clean an appearance setting for the existing CLI panes

### Delegate tasks to Antigravity alongside OpenCode

- **Completed:** 2026-09-08T12:09:37-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `fd3c652` (`feat/agy-delegation-20260908`)
- **Notes:** Delegate tasks to Antigravity alongside OpenCode

### Show delegated workers beside their principal

- **Completed:** 2026-09-08T11:37:59-05:00
- **Kind:** feature
- **Release:** v0.3.78
- **Commit:** `bceea0d` (`codex/visible-delegation-split-20260908`)
- **Notes:** feat(terminal): show delegated workers beside the principal

### Verify terminal workspace creation through the type picker

- **Completed:** 2026-09-08T09:21:51-05:00
- **Kind:** fix
- **Release:** v0.3.74
- **Commit:** `6fe038a` (`fix/workspace-picker-smoke-20260908`)
- **Notes:** fix(release): select terminal type in packaged workspace smoke

### Clear dismissed content behind glass panes

- **Completed:** 2026-09-08T07:47:13-05:00
- **Kind:** feature
- **Release:** v0.3.74
- **Commit:** `634dfd0` (`codex/glass-rendering-fixes-20260908`)
- **Notes:** Create the native glass compositor before the first paint

### Fix glass rendering and pane fade-ins

- **Completed:** 2026-09-08T07:40:42-05:00
- **Kind:** fix
- **Release:** v0.3.74
- **Commit:** `a5afdfc` (`codex/glass-rendering-fixes-20260908`)
- **Notes:** Fix glass rendering and visible-window repainting

### Create Clean or Terminal workspaces with type icons

- **Completed:** 2026-09-07T23:16:47-05:00
- **Kind:** feature
- **Release:** v0.3.74
- **Commit:** `cb57308` (`feat/clean-terminal-workspaces-20260908`)
- **Notes:** feat: create Clean or Terminal workspaces from one workspace action

### Give translucency controls room in Appearance

- **Completed:** 2026-09-07T22:53:51-05:00
- **Kind:** feature
- **Release:** v0.3.74
- **Commit:** `8ead6ba` (`codex/pane-translucency-20260907`)
- **Notes:** Give translucency sliders room in Appearance settings

### Adjust terminal and sidebar translucency

- **Completed:** 2026-09-07T22:40:40-05:00
- **Kind:** feature
- **Release:** v0.3.74
- **Commit:** `cad2608` (`codex/pane-translucency-20260907`)
- **Notes:** Add independent terminal and sidebar translucency controls

### Optional clean Code panes with inline agent activity

- **Completed:** 2026-09-07T22:17:06-05:00
- **Kind:** feature
- **Release:** v0.3.74
- **Commit:** `eb680b7` (`feat/clean-conversation-panes-20260908`)
- **Notes:** Add optional clean Code panes and inline agent activity

### Compare seven worker models on real repository tasks

- **Completed:** 2026-09-07T19:25:03-05:00
- **Kind:** feature
- **Release:** v0.3.74
- **Commit:** `79b9d34` (`codex/delegation-model-comparison-20260907`)
- **Notes:** Compare seven delegation models with real repository evidence

### Keep worker quality evidence consistent after explicit review

- **Completed:** 2026-09-07T18:17:40-05:00
- **Kind:** feature
- **Release:** v0.3.72
- **Commit:** `fa46cce` (`codex/delegation-worker-quality-20260907`)
- **Notes:** Keep worker quality evidence consistent after explicit review

### Validate delegated proposals before acceptance without worker budgets

- **Completed:** 2026-09-07T17:59:52-05:00
- **Kind:** feature
- **Release:** v0.3.72
- **Commit:** `457eecc` (`codex/delegation-worker-quality-20260907`)
- **Notes:** Validate delegated proposals and remove fixed worker budgets

### Evaluate real Codex and OpenCode on a cloned School OS task

- **Completed:** 2026-09-07T16:56:48-05:00
- **Kind:** internal
- **Release:** v0.3.72
- **Commit:** `3d24074` (`codex/delegation-real-repo-20260907`)
- **Notes:** test(delegation): evaluate real Codex work in a cloned repository

### Record delegation evaluation and package verification

- **Completed:** 2026-09-07T16:28:49-05:00
- **Kind:** record
- **Release:** v0.3.72
- **Commit:** `b5f6d35` (`codex/delegation-workflow-20260907`)
- **Notes:** Record delegation release checks and current bridge contract

### Review delegated work before integration and show worker status

- **Completed:** 2026-09-07T16:23:49-05:00
- **Kind:** feature
- **Release:** v0.3.72
- **Commit:** `dd4a6ad` (`codex/delegation-workflow-20260907`)
- **Notes:** Add pane-owned delegation decisions, review, and quality checks

### Every theme follows the Dark workspace design

- **Completed:** 2026-09-07T16:09:48-05:00
- **Kind:** fix
- **Release:** v0.3.72
- **Commit:** `6d0da33` (`fix/consistent-themes-20260907`)
- **Notes:** feat(renderer): unify themes with the Dark workspace design

### The last terminal fills its workspace after sibling panes close

- **Completed:** 2026-09-07T15:31:27-05:00
- **Kind:** fix
- **Release:** v0.3.72
- **Commit:** `fcb91cc` (`fix/terminal-single-pane-20260907`)
- **Notes:** Keep the last terminal pane full-sized after closing its siblings

### Track OpenCode Go allowance in the Usage menu

- **Completed:** 2026-09-07T15:22:05-05:00
- **Kind:** feature
- **Release:** v0.3.72
- **Commit:** `54632d1` (`feat/opencode-go-usage-20260907`)
- **Notes:** Track OpenCode Go subscription usage and reset windows

### OpenCode panes retain selected chats and recover missing resume IDs

- **Completed:** 2026-09-07T13:12:43-05:00
- **Kind:** fix
- **Release:** v0.3.71
- **Commit:** `e2277e0` (`fix/opencode-resume-recovery-20260907`)
- **Notes:** Fix OpenCode selected session capture and restart recovery

### OpenCode spend in the usage panel

- **Completed:** 2026-09-07T12:30:05-05:00
- **Kind:** fix
- **Release:** v0.3.70
- **Commit:** `e89d062` (`fix/opencode-pane-logo-20260907`)
- **Notes:** feat(usage): report OpenCode token spend in the usage panel

### OpenCode panes resume their chat across restarts

- **Completed:** 2026-09-07T12:18:35-05:00
- **Kind:** feature
- **Release:** v0.3.70
- **Commit:** `421748a` (`feat/opencode-resume-20260907`)
- **Notes:** OpenCode panes resume their chat across restarts

### OpenCode panes show working and waiting activity

- **Completed:** 2026-09-07T12:03:47-05:00
- **Kind:** fix
- **Release:** v0.3.70
- **Commit:** `4a271aa` (`codex/opencode-activity-20260907`)
- **Notes:** fix(terminal): track OpenCode activity from native TUI state

### OpenCode panes show their own logo

- **Completed:** 2026-09-07T11:49:08-05:00
- **Kind:** fix
- **Release:** v0.3.70
- **Commit:** `2477407` (`fix/opencode-pane-logo-20260907`)
- **Notes:** fix(renderer): show the official OpenCode pane logo

### OpenCode history keeps scrolling after workspace switches

- **Completed:** 2026-09-07T11:47:00-05:00
- **Kind:** fix
- **Release:** v0.3.70
- **Commit:** `6fa1a96` (`fix/opencode-scrollback-20260907`)
- **Notes:** Preserve OpenCode scrolling modes when panes remount

### OpenCode panes redraw completely after workspace switches

- **Completed:** 2026-09-07T11:33:34-05:00
- **Kind:** fix
- **Release:** v0.3.70
- **Commit:** `beb0d0c` (`fix/opencode-pane-render-20260907`)
- **Notes:** Keep terminal redraw pulses beyond OpenCode resize debounce

### Agents delegate background tasks to OpenCode Go

- **Completed:** 2026-09-07T10:51:54-05:00
- **Kind:** feature
- **Release:** v0.3.70
- **Commit:** `c0f0925` (`feat/delegate-workers-20260907`)
- **Notes:** Delegate bounded terminal tasks to OpenCode Go workers

### Release smoke verifies every terminal provider

- **Completed:** 2026-09-07T09:57:25-05:00
- **Kind:** fix
- **Release:** v0.3.68
- **Commit:** `443186f` (`codex/package-roster-smoke-20260907`)
- **Notes:** fix(release): verify the complete terminal provider roster

### Keep the terminal Fast bolt clear at small sizes

- **Completed:** 2026-09-07T09:32:29-05:00
- **Kind:** feature
- **Release:** v0.3.68
- **Commit:** `e636b54` (`codex/terminal-fast-toggle-20260907`)
- **Notes:** Keep the terminal Fast bolt legible at header size

### OpenCode is available in the terminal provider picker

- **Completed:** 2026-09-07T09:27:39-05:00
- **Kind:** feature
- **Release:** v0.3.68
- **Commit:** `7e353ab` (`feat/opencode-terminal`)
- **Notes:** Add OpenCode to integrated terminal profiles

### Toggle Fast mode from the terminal header

- **Completed:** 2026-09-07T09:23:20-05:00
- **Kind:** feature
- **Release:** v0.3.68
- **Commit:** `514ef0d` (`codex/terminal-fast-toggle-20260907`)
- **Notes:** Add a live Fast mode toggle beside the terminal pane menu

### Agents have distinct characters and a searchable vertical menu

- **Completed:** 2026-09-07T09:08:50-05:00
- **Kind:** feature
- **Release:** v0.3.68
- **Commit:** `43f123c` (`feat/agent-roster-20260907`)
- **Notes:** Give agents shaped characters and a searchable vertical roster

### Chat resumes reliably and shows only supported controls

- **Completed:** 2026-09-07T09:01:03-05:00
- **Kind:** fix
- **Release:** v0.3.68
- **Commit:** `e02cd26` (`codex/chat-review-20260907`)
- **Notes:** fix(chat): align controls and resume with provider capabilities

### Agent workspace keeps chat visible and offers provider model choices

- **Completed:** 2026-09-07T08:47:13-05:00
- **Kind:** feature
- **Release:** v0.3.68
- **Commit:** `69ec21b` (`feat/agent-workspace-20260907`)
- **Notes:** Begin agent workspace with visible chat and provider model choices

### OMP is available in the terminal provider picker

- **Completed:** 2026-09-07T08:39:57-05:00
- **Kind:** feature
- **Release:** v0.3.68
- **Commit:** `7d26d6d` (`feat/omp-terminal`)
- **Notes:** Add OMP to the terminal provider picker

### Preserve wrapped task text in terminal cards

- **Completed:** 2026-09-06T23:54:06-05:00
- **Kind:** feature
- **Release:** v0.3.66
- **Commit:** `922e9c7` (`codex/gui-consistency-20260906`)
- **Notes:** Preserve wrapped task text in terminal cards

### Keep worker forms and terminal summaries readable

- **Completed:** 2026-09-06T23:48:47-05:00
- **Kind:** feature
- **Release:** v0.3.66
- **Commit:** `88618d0` (`codex/gui-consistency-20260906`)
- **Notes:** Keep worker forms and terminal summaries readable

### Polish canvas, search, terminal cards, and entry screens

- **Completed:** 2026-09-06T23:38:42-05:00
- **Kind:** feature
- **Release:** v0.3.66
- **Commit:** `674bc34` (`codex/gui-consistency-20260906`)
- **Notes:** Polish canvas, search, terminal cards, and entry screens

### A newer release prevents an older candidate from staging

- **Completed:** 2026-09-06T23:23:09-05:00
- **Kind:** feature
- **Release:** v0.3.66
- **Commit:** `8d0b046` (`codex/streamline-release-pipeline`)
- **Notes:** Stop superseded releases after publication readback

### Terminal completions notify and open the right workspace

- **Completed:** 2026-09-06T23:19:49-05:00
- **Kind:** fix
- **Release:** v0.3.66
- **Commit:** `0d76d65` (`fix/terminal-completion-notifications`)
- **Notes:** fix(notifications): deliver terminal completion alerts

### Dev updates verify automatically and releases resume without rebuilding

- **Completed:** 2026-09-06T23:18:23-05:00
- **Kind:** feature
- **Release:** v0.3.66
- **Commit:** `ebf6569` (`codex/streamline-release-pipeline`)
- **Notes:** Streamline dev verification and resumable release promotion

### Terminal streaming uses less CPU and review commands respect timeouts

- **Completed:** 2026-09-06T22:59:49-05:00
- **Kind:** speed
- **Release:** v0.3.66
- **Commit:** `22c735a` (`codex/performance-review-20260906`)
- **Notes:** perf: bound terminal transport and subprocess resource costs

### Workspace selections keep their sidebar position

- **Completed:** 2026-09-06T22:39:53-05:00
- **Kind:** fix
- **Release:** v0.3.66
- **Commit:** `df79987` (`fix/sidebar-workspace-position`)
- **Notes:** fix(sidebar): preserve workspace position when selected

### Named landings on dev and promote --change to ship them one at a time

- **Completed:** 2026-09-06T20:48:33-05:00
- **Kind:** feature
- **Release:** v0.3.66
- **Commit:** `c6f813d` (`feat/per-landing-promotion`)
- **Notes:** docs(xtask): per-landing promotion design; docs(xtask): per-landing promotion implementation plan; feat(xtask): landing name rule and message; feat(xtask): parse, filter, resolve and render landings; fix(xtask): short landing SHAs through gate::short; feat(xtask): closeout lands one named merge commit on dev; feat(xtask): landings lists what dev has that main lacks; feat(xtask): promote --change ships chosen landings to main; fix(xtask): restore HEAD on every selective-promote failure path; docs: closeout --change, landings, and promote --change; fix(xtask): pin the dev tip a closeout merges and lands on; fix(xtask): put HEAD back on every selective-promote failure; docs(xtask): name the rollout escape hatch and refuse an empty --change; docs(spec): describe closeout as the fix wave leaves it

