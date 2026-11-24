import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  Conversation, createQueueEditingForwarder, type ConversationProps,
} from "./Conversation";
import {
  effectiveProviderCommands, loadPrelaunchProviderCommands, prelaunchCommandScope,
} from "./ConversationComposer";
import {
  questionAnswersReady, sameConversationItemProps, sameSubagentGridProps, sameWorkRowProps,
  toggleQuestionOption, type ConversationItemContext, type FindRenderContext,
} from "./ConversationRows";
import {
  collapseSuccessfulToolRuns, displayRowKeys, groupSubagentGrids,
} from "../conversation-display";
import {
  conversationEscapeAction, conversationSteerKeyAction, conversationSubmitAction,
  draftAfterFailedSend, finishedTurnAnnouncement, queueSubmissionAccepted,
} from "../conversation-pane-rules";
import { createDraftStore } from "../draft-store";
import { jumpLatestBacklog } from "../transcript-navigation";
import { toolRunMinimum } from "../transcript-density";
import { workGroupParts, workGroupPartsLabel } from "../tool-activity";
import type { ConvItem } from "../conversation-reducer";
import type { TranscriptRow } from "../transcript-merge";
import type {
  ProviderCommand, ProviderCommandCatalogue, ProviderCommandScope,
} from "../../shared/engine";
import type { PromptContextItem } from "../prompt-context";
import { promptSubmitKeyAction } from "../prompt-submit";

const context: PromptContextItem = {
  id: "context-app", kind: "file", label: "App.tsx", value: "/repo/App.tsx",
};

const baseProps: ConversationProps = {
  items: [],
  thinking: false,
  live: true,
  config: {
    effort: null,
    models: [],
    canSetModel: false,
    effortLevels: [],
    showEffort: false,
    canSetEffort: false,
    pendingModel: false,
    pendingEffort: false,
    error: null,
  },
  onSend: () => undefined,
  onStop: () => undefined,
  onSetModel: () => undefined,
  onSetEffort: () => undefined,
  onApprove: () => undefined,
  onDeny: () => undefined,
  approvals: {},
  activeApprovalIds: new Set<string>(),
};

/** The pane's draft, as a caller's store holds it for the keyless pane. */
const drafted = (text: string) => createDraftStore({ initial: { "": text } });

const conversationCss = readFileSync(new URL("./Conversation.css", import.meta.url), "utf8");
const read = (file: string): string => readFileSync(new URL(`./${file}`, import.meta.url), "utf8");
const shellSource = read("Conversation.tsx");
const transcriptSource = read("ConversationTranscript.tsx");
const composerSource = read("ConversationComposer.tsx");
const rowsSource = read("ConversationRows.tsx");
/** The pane's whole source, read once: the shell, its two halves and the rows. */
const source = [shellSource, transcriptSource, composerSource, rowsSource].join("\n");

describe("Conversation durable queue dispatch", () => {
  it("accepts file and image drops on the conversation composer", () => {
    // The pane spreads the drop zone it shares with the launch card over its
    // whole root; prompt-drop.test.ts drives its paths, images and refusals.
    expect(shellSource).toContain("const dropZone = promptDropZone({");
    expect(shellSource).toContain("onDroppedPaths: props.onDroppedPaths,");
    expect(shellSource).toContain("pathForFile: props.pathForFile,");
    expect(shellSource).toContain("{...dropZone}>");
    expect(source).toContain("Drop files to attach");
  });

  it("wraps a user turn's words in a span the chrome can hang a caret on", () => {
    // CLI chrome draws Claude Code's `›` before the message. It cannot hang on
    // .conv-turn-user, whose first child is the "You" head row, so the words
    // need an element of their own. An inline span inherits everything, which
    // is why refined chrome cannot tell the difference — and why this is a
    // source pin rather than a rendered-appearance assertion.
    expect(source).toContain('<span className="conv-turn-text">{item.text}</span>');
  });

  it("uses the same rounded, full-width prompt language as new sessions", () => {
    const markup = conversationCss;
    expect(markup).toContain("width:min(calc(100% - 2 * var(--conv-gutter)), var(--conv-measure))");
    expect(markup).toContain("border-radius:var(--radius-composer)");
    expect(markup).toContain("font-size:var(--text-prose); line-height:var(--lh-ui)");
    // The accent, not the selection blue: theme.css reserves the blue for
    // selection and focus, and Send is a primary action like the queue's Save
    // one step away from it.
    expect(markup).toContain(".conv-send { color:var(--accent-fg); background:var(--accent); border-radius:50%");
    expect(markup).toContain(
      ".conv-send:hover:not(:disabled) {\n  color:var(--accent-fg); background:var(--accent-hover);",
    );
  });

  it("renders the provider plan as a first-class progress artifact", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      plan: [
        { content: "Inspect repository", priority: "high", status: "completed" },
        { content: "Run checks", priority: "medium", status: "in_progress" },
      ],
    }));
    expect(markup).toContain("Task plan");
    expect(markup).toContain("1/2 complete");
    expect(markup).toContain("Inspect repository");
    expect(markup).toContain("Run checks");
  });

  it("offers only this live provider's advertised slash commands", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      drafts: drafted("/"),
      commands: [
        { name: "review", description: "Review changes", argumentHint: "<target>" },
        { name: "compact", description: "Compact context" },
      ],
      config: { ...baseProps.config, engine: "acp:grok", engineLabel: "Grok" },
      onRunCommand: () => undefined,
    }));
    expect(markup).toContain('aria-label="Grok commands"');
    expect(markup).toContain('role="listbox"');
    expect(markup).toContain('aria-haspopup="listbox"');
    expect(markup).toContain('aria-activedescendant=');
    expect(markup).toContain("/review");
    expect(markup).toContain("/compact");
    expect(markup).toContain("Enter run");
    expect(source).toContain("draft.trim().length > 0");
    expect(source).toContain("Clear the current draft before choosing a provider command");
    expect(source).toContain("aria-disabled={commandLauncherReason !== null || undefined}");
    expect(source).toContain("commands unavailable. ${commandLauncherReason}");
    expect(source).toContain("Provider commands are never queued");
    expect(source).toContain("Slash commands are never queued or sent with prompt context");
    expect(source).toContain("canonicalProviderCommandText");
    // The menu's keys, rows and commit are the ones the launch card uses too,
    // pressed in conversation.test.ts ("the slash menu, pressed").
    expect(composerSource).toContain("const commandMenu = useProviderCommandMenu({");
    expect(composerSource).toContain("if (commandMenu.onKeyDown(e)) return;");
    expect(source).toContain("onBlur={() => { commandMenu.close(); }}");
  });

  it("keeps the slash launcher on a first chat before the catalogue arrives", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      items: [],
      config: { ...baseProps.config, engine: "claude", engineLabel: "Claude",
        canSetModel: true, model: "claude-opus-5" },
      onRunCommand: () => undefined,
    }));
    expect(markup).toContain('aria-label="Claude commands"');
    expect(markup).not.toContain("Choose a model before sending");
  });

  it("states when provider history is unavailable without offering a false retry", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      history: { status: "unavailable", message: "Grok ACP does not expose replay." },
      onRetryHistory: () => undefined,
    }));
    expect(markup).toContain("Grok ACP does not expose replay.");
    expect(markup).not.toContain(">Retry<");
    expect(markup).not.toContain("No messages yet.");
  });

  it("routes only nonblank busy drafts into an available queue", () => {
    expect(conversationSubmitAction("  ", true, true)).toBe("ignore");
    expect(conversationSubmitAction("Follow up", true, true)).toBe("queue");
    expect(conversationSubmitAction("Follow up", true, false)).toBe("send");
    expect(conversationSubmitAction("Follow up", false, true)).toBe("send");
    expect(conversationSubmitAction("Follow up", false, true, true)).toBe("ignore");
  });

  it("resolves an explicit steer to the send path even with a queue available", () => {
    // The ONLY difference steer makes: the durable queue stops being the answer
    // for a busy composer. Every other refusal still stands ahead of it.
    expect(conversationSubmitAction("Follow up", true, true, false, true)).toBe("send");
    expect(conversationSubmitAction("Follow up", true, true)).toBe("queue");
    expect(conversationSubmitAction("  ", true, true, false, true)).toBe("ignore");
    expect(conversationSubmitAction("Follow up", true, true, true, true)).toBe("ignore");
  });

  it("routes Alt/Option+Enter to steer only where the control is offered", () => {
    const alt = { key: "Enter", shiftKey: false, altKey: true };
    expect(conversationSteerKeyAction(alt, true)).toBe("steer");
    // No capability, no chord: the engine that cannot take mid-turn input must
    // not have a second way to try.
    expect(conversationSteerKeyAction(alt, false)).toBe("ignore");
    expect(conversationSteerKeyAction({ key: "Enter", shiftKey: false }, true)).toBe("ignore");
    expect(conversationSteerKeyAction({ key: "Enter", shiftKey: true, altKey: true }, true))
      .toBe("ignore");
    expect(conversationSteerKeyAction({ ...alt, isComposing: true }, true)).toBe("ignore");
    expect(conversationSteerKeyAction({ ...alt, nativeEvent: { keyCode: 229 } }, true))
      .toBe("ignore");
    // And nothing about the chord's meaning changed anywhere else.
    expect(promptSubmitKeyAction(alt)).toBe("newline");
  });

  it("offers Steer only where the engine proved it takes mid-turn input", () => {
    const busy = (over: Partial<ConversationProps>): string =>
      renderToStaticMarkup(createElement(Conversation, {
        ...baseProps,
        thinking: true,
        drafts: drafted("Also run the linter"),
        onQueue: () => true,
        ...over,
      }));
    const steerable = busy({ canSteer: true });
    expect(steerable).toContain('aria-label="Steer" title="Delivered into the running turn"');
    expect(steerable).toContain(">Steer<");
    // Enter is untouched: the primary control still says Queue, and Steer is
    // the second verb standing beside it.
    expect(steerable).toContain('aria-label="Queue after current turn"');
    // Grok/Qwen and every ACP profile: the flag is absent, so the control is
    // not there at all rather than there and quietly queueing.
    expect(busy({})).not.toContain(">Steer<");
    expect(busy({ canSteer: false })).not.toContain(">Steer<");
    // An idle composer has no running turn to steer into.
    expect(busy({ canSteer: true, thinking: false, queueWhileBusy: false }))
      .not.toContain(">Steer<");
  });

  it("disables Steer while a queued prompt is already being delivered", () => {
    const blocked = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      thinking: true,
      drafts: drafted("Also run the linter"),
      canSteer: true,
      onQueue: () => true,
      queueSendingId: "queued-1",
    }));
    expect(blocked).toContain('class="conv-iconbtn conv-steer" disabled=""');
    expect(blocked).toContain('title="Another prompt is already being delivered."');
    expect(blocked).toContain('aria-label="Steer unavailable. '
      + 'Another prompt is already being delivered."');
  });

  it("never lets the chord eat a newline it is not going to steer with", () => {
    // THE DEFECT: the chord was admitted on `steerOffered && !blocked`, which
    // says nothing about the draft or the lock. On a busy tile with an empty
    // composer, Alt+Enter preventDefaulted and then dispatched to "ignore" —
    // no steer, and no newline either. The chord and the button now read the
    // same predicate, so the chord is live exactly when the verb is.
    expect(source).toContain("const steerReady = steerOffered && steerBlockedReason === null\n"
      + "    && !composerLocked && draft.trim() !== \"\";");
    expect(source).toContain("if (conversationSteerKeyAction(e, steerReady) === \"steer\") {");
    expect(source).toContain("disabled={!steerReady}");

    // And the pure rule still refuses everything a caller hands it as false,
    // which is what makes one predicate enough.
    const alt = { key: "Enter", shiftKey: false, altKey: true };
    expect(conversationSteerKeyAction(alt, false)).toBe("ignore");
    // The blocked and empty-draft states the predicate now covers, sampled
    // through the control that shows them: disabled, so no dispatch either way.
    const busy = (over: Partial<ConversationProps>): string =>
      renderToStaticMarkup(createElement(Conversation, {
        ...baseProps, thinking: true, canSteer: true, onQueue: () => true, ...over,
      }));
    expect(busy({ drafts: drafted("") })).toContain('class="conv-iconbtn conv-steer" disabled=""');
    expect(busy({ drafts: drafted("Also run the linter"), sendLocked: true }))
      .toContain('class="conv-iconbtn conv-steer" disabled=""');
    expect(busy({ drafts: drafted("Also run the linter") }))
      .toContain('class="conv-iconbtn conv-steer" aria-label="Steer"');
  });

  it("tells the tile it can steer from the probe, not from the engine's name", () => {
    // Steer's dispatch through the send path is pressed in conversation.test.ts
    // ("steer and stop, pressed").
    const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
    // The tile is told by the PROBE, never by the engine's name.
    expect(app).toContain("?.info?.capabilities.steer === true");
    expect(app).toContain("canSteer={canSteerEngine(summary.engine)}");
  });

  it("locks every send path while provider settings are unconfirmed", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      sendLocked: true,
      sendLockReason: "Applying session settings…",
      config: {
        ...baseProps.config,
        canSetModel: true,
        model: "next-model",
      },
    }));
    expect(markup).toContain('class="conv-ta" rows="1" disabled="" aria-busy="true" aria-describedby=');
    // The reason belongs to the banner below and the send button's tooltip, and
    // to neither of them twice: it used to be the placeholder as well, which
    // stacked the identical sentence in the primary control.
    expect(markup).toContain('aria-label="Message" placeholder="Message this session…"');
    expect(markup).not.toContain('placeholder="Applying session settings…"');
    expect(markup).toContain('conv-config-select conv-model-select"');
    expect(markup).toContain('class="app-select-trigger" disabled="" aria-label="Model: next-model"');
    expect(markup).toContain('class="conv-lock-reason" role="status"');
    expect(markup).toContain('aria-describedby=');
    expect(markup).toContain('aria-label="Send" title="Applying session settings…"');
  });

  it("offers a prompt immediately on a paused session and reconnects on send", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      live: false,
      config: {
        ...baseProps.config,
        model: "claude-sonnet",
        canSetModel: true,
        canSetEffort: true,
        showEffort: true,
        effort: "high",
        effortLevels: ["low", "high"],
        canSetPermission: true,
        permissionMode: "acceptEdits",
        permissionModes: ["default", "acceptEdits", "plan"],
        models: [{
          value: "claude-sonnet", displayName: "Sonnet 5",
          supportsEffort: true, effortLevels: ["low", "high"],
        }],
      },
    }));
    expect(markup).toContain('aria-haspopup="listbox"');
    expect(markup).toContain('aria-label="Model: Sonnet 5"');
    expect(markup).toContain('<span class="app-select-value">Sonnet 5</span>');
    expect(markup).not.toContain("claude-sonnet");
    expect(markup).toContain('class="app-select-leading"');
    expect(markup).toContain('aria-label="Message"');
    expect(markup).toContain("Message this session");
    expect(markup).toContain("Enter to send");
    expect(markup).not.toContain("Cmd+Enter");
    expect(markup).not.toContain("Continue session");
    expect(markup).toContain('conv-config-select conv-effort-select');
    expect(markup).toContain('aria-label="Effort: High"');
    expect(markup).toContain('aria-haspopup="dialog"');
    expect(markup).not.toContain('role="slider"');
    // The access mode is the pane header's now, so the control row no longer
    // carries it however the transport answers about permissions.
    expect(markup).not.toContain('aria-label="Access:');
    expect(markup).not.toContain("Automatic");
    expect(markup).not.toContain('aria-label="Stop"');
  });

  it("shows the catalogue name when the session reports a bare Claude id", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      live: true,
      config: {
        ...baseProps.config,
        model: "claude-fable-5",
        canSetModel: true,
        models: [{
          value: "opus[1m]", displayName: "Opus 5 (1M context)",
          supportsEffort: false, effortLevels: [],
        }, {
          value: "claude-fable-5[1m]", displayName: "Fable 5",
          description: "Fable 5 · Most capable for your hardest and longest-running tasks",
          supportsEffort: true, effortLevels: ["low", "high"],
        }, {
          value: "sonnet", displayName: "Sonnet 5",
          supportsEffort: false, effortLevels: [],
        }],
      },
    }));
    expect(markup).toContain('aria-label="Model: Fable 5"');
    expect(markup).toContain('<span class="app-select-value">Fable 5</span>');
    expect(markup).not.toContain(">claude-fable-5<");
    expect(markup).not.toContain('aria-label="Model: claude-fable-5"');
  });

  it("requires a concrete model before a paused mutable session can send", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      live: false,
      drafts: drafted("Continue the work"),
      config: {
        ...baseProps.config,
        canSetModel: true,
        models: [{
          value: "default", displayName: "Automatic",
          supportsEffort: false, effortLevels: [],
        }, {
          value: "claude-sonnet", displayName: "Claude Sonnet",
          supportsEffort: true, effortLevels: ["low", "high"],
        }],
      },
    }));
    expect(markup).toContain('aria-label="Model: Choose model"');
    expect(markup).toContain('<span class="app-select-value app-select-placeholder">Choose model</span>');
    expect(markup).toContain('placeholder="Message this session…"');
    expect(markup).not.toContain('placeholder="Choose a model before sending…"');
    expect(markup).toContain('aria-label="Send" title="Choose a model before sending…"');
    expect(markup).not.toContain("Automatic");
  });

  it("does not lock send on a model-less session that already has transcript", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      items: [{ kind: "text", id: "t1", text: "hello", streaming: false }],
      config: {
        ...baseProps.config,
        canSetModel: true,
      },
    }));
    expect(markup).not.toContain('class="conv-lock-reason"');
    expect(markup).not.toContain("Choose a model before sending…");
    expect(markup).toContain("Message this session…   (Enter to send)");
  });

  it("keeps a refused queued draft and accepts void fire-and-forget callbacks", () => {
    expect(queueSubmissionAccepted(false)).toBe(false);
    expect(queueSubmissionAccepted(true)).toBe(true);
    expect(queueSubmissionAccepted(undefined)).toBe(true);
  });

  it("gives a definitely failed send its words back", () => {
    // Dispatch cleared the composer, so an empty draft is the one state where
    // a message that never left the app belongs back in it. The pane pressed
    // through a refusal: conversation.test.ts, "a refused send, pressed".
    expect(draftAfterFailedSend("", "Run the checks")).toBe("Run the checks");
  });

  it("never clobbers text typed while the failed send was in flight", () => {
    expect(draftAfterFailedSend("a newer thought", "Run the checks")).toBe("a newer thought");
    expect(draftAfterFailedSend("Run the checks", "Run the checks")).toBe("Run the checks");
  });

  it("renders a bounded queue, visible error, compact context, and busy send label", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      thinking: true,
      queueItems: [{ id: "queued-1", text: "Run checks", context: [context] }],
      queueError: "Queue is full. Remove a prompt before adding another.",
      onQueue: () => true,
      onQueueMove: () => undefined,
      onQueueEdit: () => undefined,
      onQueueRemove: () => undefined,
      onQueueSendNow: () => undefined,
      onQueueClear: () => undefined,
    }));
    expect(markup).toContain("Queued follow-ups");
    expect(markup).toContain('class="conv-queue-error" role="alert"');
    expect(markup).toContain("Queue is full.");
    expect(markup).toContain('aria-label="1 attached context item"');
    expect(markup).toContain('aria-label="Queue after current turn"');
    expect(markup).toContain("Queue after current turn…");
    // REFINED keeps the word and the clock in two spans. Both are aria-hidden
    // now — the clock reticks every second inside a polite live region — and
    // one static word is announced beside them instead.
    expect(markup).toContain('class="conv-working-label" aria-hidden="true">Working<');
    expect(markup).not.toContain("Working…");
    expect(markup).toContain('<span class="conv-sr">Working</span>');
    expect(markup).toContain('data-provider="custom"');
    expect(markup).toContain("conv-working-mark");
    expect(markup).toContain('class="conv-working-elapsed"');
    expect(markup).toContain('class="conv-working-elapsed" aria-hidden="true"');
    expect(markup).toContain("0s");
    expect(markup).not.toContain("conv-working-inline");
    expect(markup.match(/class="conv-working"/g)).toHaveLength(1);
  });

  it("renders structured questions instead of Allow/Deny", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      live: true,
      items: [{
        kind: "approval",
        id: "q1",
        tool: "AskUserQuestion",
        summary: "AskUserQuestion",
        input: {},
        questions: [{
          question: "Which library?",
          header: "Library",
          multiSelect: false,
          options: [
            { label: "date-fns", description: "small" },
            { label: "luxon", description: "zones" },
          ],
        }],
      }],
      activeApprovalIds: new Set(["q1"]),
    }));
    expect(markup).toContain("Which library?");
    expect(markup).toContain("date-fns");
    expect(markup).toContain('role="radiogroup"');
    expect(markup).toContain('type="radio"');
    expect(markup).not.toContain('aria-pressed=');
    expect(markup).toContain("Send answers");
    expect(markup).not.toContain(">Allow<");
  });

  it("preserves a multi-select Other answer and requires its text", () => {
    const question = {
      question: "Which checks?", header: "Checks", multiSelect: true,
      options: [
        { label: "Unit", description: "Fast" },
        { label: "End to end", description: "Broad" },
      ],
    };
    const withOther = toggleQuestionOption([], "Other", true);
    expect(withOther).toEqual({ selected: ["Other"], clearOther: false });
    const withUnit = toggleQuestionOption(withOther.selected, "Unit", true);
    expect(withUnit).toEqual({ selected: ["Other", "Unit"], clearOther: false });
    expect(questionAnswersReady([question], { "Which checks?": withUnit.selected }, {
      "Which checks?": "",
    })).toBe(false);
    expect(questionAnswersReady([question], { "Which checks?": withUnit.selected }, {
      "Which checks?": "Performance suite",
    })).toBe(true);
    expect(toggleQuestionOption(withUnit.selected, "Other", true).clearOther).toBe(true);
  });

  it("fails closed with an explicit fallback when a question payload is malformed", () => {
    expect(questionAnswersReady([], {}, {})).toBe(false);
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      live: true,
      items: [{
        kind: "approval", id: "q-bad", tool: "AskUserQuestion",
        summary: "AskUserQuestion", input: {}, questions: [],
      }],
      activeApprovalIds: new Set(["q-bad"]),
    }));
    expect(markup).toContain("Question details are unavailable");
    expect(markup).toContain("Send answers");
    expect(markup).toContain('disabled=""');
    expect(markup).toContain(">Skip<");
  });

  /* Both blocking cards render one shared wrapper (ApprovalShell) over one
     shared verdict hook (useApprovalVerdict). This pins the wrapper byte for
     byte — attribute set AND order — so the shared scaffold cannot drift from
     what the two cards used to emit inline. tabindex=-1 is the focus target
     the settle effect returns to; the alert role and its label exist only
     while the request is genuinely blocking this pane. data-voice-guard is on
     every one of them, blocking or not: it is what makes the voice assistant's
     ui_act refuse this card, and a card that lost it would answer to the
     assistant instead of the user. */
  it("wraps both blocking cards in one focusable, labelled alert", () => {
    const pending = (item: ConvItem, ids: string[]): string =>
      renderToStaticMarkup(createElement(Conversation, {
        ...baseProps, live: true, items: [item], activeApprovalIds: new Set(ids),
      }));
    const approval: ConvItem = {
      kind: "approval", id: "ap", tool: "Bash", summary: "run it", input: {},
    };
    const question: ConvItem = {
      kind: "approval", id: "q", tool: "AskUserQuestion", summary: "AskUserQuestion",
      input: {}, questions: [],
    };

    expect(pending(approval, ["ap"])).toContain(
      '<div tabindex="-1" class="conv-approval conv-approval-pending" role="alert"'
      + ' aria-label="Approval required" data-voice-guard="approval">');
    expect(pending(question, ["q"])).toContain(
      '<div tabindex="-1" class="conv-approval conv-approval-pending" role="alert"'
      + ' aria-label="The agent is asking a question" data-voice-guard="approval">');

    // Not blocking this pane: still focusable, no alert and no label to announce.
    for (const item of [approval, question]) {
      const closed = pending(item, []);
      expect(closed).toContain(
        '<div tabindex="-1" class="conv-approval" data-voice-guard="approval">');
      expect(closed).not.toContain('aria-label="Approval required"');
      expect(closed).not.toContain('aria-label="The agent is asking a question"');
    }
  });

  it("labels the busy strip with the tool summary the shell passed", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      thinking: true,
      workingLabel: "Read `/repo/App.tsx`",
    }));
    expect(markup).toContain('class="conv-working-label" aria-hidden="true">Read `/repo/App.tsx`<');
    expect(markup).not.toContain("Read `/repo/App.tsx`…");
    expect(markup).toContain("conv-working-mark");
    expect(markup).toContain('class="conv-working-elapsed"');
    expect(markup).not.toContain(">Working…<");
    // The ANNOUNCED word stays the static one: the strip is a polite live
    // region and a per-frame label would re-announce the sentence each second.
    expect(markup).toContain('class="conv-sr">Working<');
  });

  it("falls back to Working when the shell names no live activity", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      thinking: true,
    }));
    expect(markup).toContain('class="conv-working-label" aria-hidden="true">Working<');
  });

  it("keeps Claude Code's busy and settled wording behind the CLI gate", () => {
    // The markup above IS the proof for refined: this suite runs in vitest's
    // node environment, where useDocumentChrome finds no document and answers
    // "refined", so the CLI branch never renders here. It is pinned at the one
    // place it can be — the source — because the failure it guards against is
    // exactly the one that shipped: helpers called unconditionally, and
    // "Turn finished · 2m 14s" appearing in a chrome that never asked for it.
    expect(source).toContain("{cliChrome ? (");
    expect(source).toContain("{workingLineText(elapsedMs, undefined, label)}");
    expect(source).toContain("function LiveTurnElapsed");
    expect(source).toContain('<span className="conv-working-label" aria-hidden="true">{workingWord}</span>');
    // Rust collapsed the three settled clocks into TurnClock; the CLI branch
    // lives there, not re-inlined at each call site.
    expect(source).toContain("{cliChrome ? doneLineText(ms) : formatTurnElapsed(ms)}");
    expect(source.match(/<TurnClock ms=\{/gu)).toHaveLength(3);
    expect(source).toContain("cliChrome={cliChrome}");
    // Threaded by identity, never spread — and the identity is the pane's one
    // memoized ConversationItemContext rather than React's per-render `props`,
    // which no row memo could ever compare equal.
    expect(source).toContain("<SearchableItem item={row.item} all={itemContext}");
    // Every transcript row is a memo boundary: a composer keystroke re-renders
    // the pane, and its rows' props (memoized displayRows, itemContext,
    // findRender, scalars) are all identity-stable across it.
    for (const row of ["SearchableItem", "ToolRun", "FileChangeRow", "EditedFilesRow", "SubagentGrid"]) {
      expect(source).toContain(`const ${row} = memo(function ${row}(`);
    }
    expect(source).not.toContain("<SearchableItem item={row.item} {...props}");
    expect(source).not.toContain("<SearchableItem item={row.item} all={props}");
    expect(source).not.toMatch(/\n\s*\{doneLineText\(/u);
    // The opening block is CLI's alone, so refined does not even compute it.
    expect(source).toContain("const welcome = cliChrome");
    expect(source).toContain("{cliChrome && welcome !== null && welcome.title !== \"\" && (");
  });

  it("preserves provider command output lines in transcript notices", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      items: [{ kind: "notice", id: "status", tone: "info",
        text: "Codex status:\n- Model: current\n- Directory: /repo" }],
    }));
    expect(markup).toContain("Codex status:\n- Model: current\n- Directory: /repo");
    expect(conversationCss).toContain(".conv-notice > span { min-width:0; white-space:pre-wrap;");
  });

  it("draws an imported session's markers as rows, not as prose", () => {
    // The pane is what proves expandImportedRows is actually wired into `rows`
    // — the split itself is unit-tested in conversation.test.ts.
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      items: [
        { kind: "text", id: "text-0", streaming: false,
          text: "<EXTERNAL SESSION IMPORTED>" },
        { kind: "text", id: "text-1", streaming: false, text:
          "[external_agent_tool_call: Bash]\ndescription: List the tests"
          + "\ncommand: ls tests\n[/external_agent_tool_call]" },
      ],
    }));
    expect(markup).toContain('class="conv-mark">Imported session</div>');
    expect(markup).toContain('class="conv-name">Bash</span>');
    expect(markup).toContain("List the tests");
    expect(markup).not.toContain("external_agent_tool_call");
  });

  it("blocks provider commands while prompt context is attached", () => {
    const guard = source.indexOf("commandLike !== null && contextItems.length > 0");
    const dispatch = source.indexOf("const sending = runsAsCommand", guard);
    expect(guard).toBeGreaterThan(-1);
    expect(dispatch).toBeGreaterThan(guard);
    expect(source).toContain("Provider commands cannot consume files, diffs, or prior-chat attachments");
    expect(source.indexOf("const commandLike = parseProviderCommandLike(text)"))
      .toBeLessThan(guard);
  });

  it("can queue an approval-waiting turn without claiming the agent is working", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      thinking: false,
      queueWhileBusy: true,
      onQueue: () => undefined,
    }));
    expect(markup).toContain('aria-label="Queue after current turn"');
    expect(markup).not.toContain("Working…");
  });

  it("places selected context above the draft and its picker before config controls", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      contextItems: [context],
      contextCapabilities: { file: { available: true } },
      contextOpen: true,
      onContextOpenChange: () => undefined,
      onContextRequest: () => undefined,
      onContextRemove: () => undefined,
      onContextConsumed: () => undefined,
    }));
    expect(markup).toContain('class="conv-context-row"');
    expect(markup).toContain('aria-label="Attached prompt context"');
    expect(markup).toContain("App.tsx");
    expect(markup).toContain('role="menu" aria-label="Add prompt context"');
    expect(markup.indexOf('class="prompt-context-trigger"'))
      .toBeLessThan(markup.indexOf('class="conv-pills"'));
  });

  it("splits a long run around the calls that failed or are still pending", () => {
    const tool = (key: string, name: string, ok: boolean | "running" = true): TranscriptRow => ({
      kind: "item",
      key,
      origin: "live",
      item: {
        kind: "tool",
        id: key,
        tool: name,
        summary: `${name} ${key}`,
        input: { path: `/repo/${key}` },
        result: ok === "running" ? undefined : { ok, summary: ok ? "done" : "failed" },
      },
    });
    const rows: TranscriptRow[] = [
      tool("read-1", "read_file"),
      tool("read-2", "read_file"),
      tool("read-3", "read_file"),
      tool("failed", "read_file", false),
      tool("pending", "read_file", "running"),
      tool("write-1", "write_file"),
      tool("write-2", "write_file"),
    ];

    // A fold is a claim that nothing in it needs attention, so the two calls
    // that do stay on their own rows and the stretch splits around them. The
    // three reads before them still fold; the edits after them are exempt for
    // the separate reason pure-edit stretches always were.
    const collapsed = collapseSuccessfulToolRuns(rows);
    expect(collapsed[0]).toMatchObject({ kind: "tool-run", tool: "work" });
    if (collapsed[0]?.kind !== "tool-run") throw new Error("expected a grouped tool run");
    expect(collapsed[0].rows.map(row => row.key)).toEqual(["read-1", "read-2", "read-3"]);
    expect(collapsed.slice(1).map(entry =>
      entry.kind === "row" ? entry.row.key : entry.kind))
      .toEqual(["failed", "pending", "write-1", "write-2"]);
  });

  it("folds a run of one at compact and leaves it a row at every other step", () => {
    // The dial's three steps have to be three shapes. A lone inspection call is
    // the case that used to make compact and balanced identical: both left it
    // as its own row, so the leftmost step of the dial did nothing.
    const read: TranscriptRow = {
      kind: "item", key: "r1", origin: "live",
      item: { kind: "tool", id: "r1", tool: "Read", summary: "read a.ts",
        input: { file_path: "/repo/a.ts" }, result: { ok: true, summary: "ok" } },
    };
    expect(collapseSuccessfulToolRuns([read], toolRunMinimum("balanced"))
      .map(entry => entry.kind)).toEqual(["row"]);
    expect(collapseSuccessfulToolRuns([read], toolRunMinimum("detailed"))
      .map(entry => entry.kind)).toEqual(["row"]);
    const compact = collapseSuccessfulToolRuns([read], toolRunMinimum("compact"));
    expect(compact).toHaveLength(1);
    expect(compact[0]).toMatchObject({ kind: "tool-run", tool: "work" });
    if (compact[0]?.kind !== "tool-run") throw new Error("expected a work run");
    expect(compact[0].rows.map(row => row.key)).toEqual(["r1"]);
  });

  it("folds a stretch of pure edits at compact and merges them per file otherwise", () => {
    const edit = (key: string, path: string): TranscriptRow[] => [
      { kind: "item", key, origin: "live",
        item: { kind: "tool", id: key, tool: "Edit", summary: `edit ${path}`,
          input: { file_path: path }, result: { ok: true, summary: "ok" } } },
      { kind: "item", key: `d-${key}`, origin: "live",
        item: { kind: "diff", id: `d-${key}`, callId: key, path, added: 3, removed: 1 } },
    ];
    const rows = [...edit("e1", "/repo/a.ts"), ...edit("e2", "/repo/b.ts")];
    for (const density of ["balanced", "detailed"] as const) {
      expect(collapseSuccessfulToolRuns(rows, toolRunMinimum(density))
        .map(entry => entry.kind)).toEqual(["file-change", "file-change"]);
    }
    const compact = collapseSuccessfulToolRuns(rows, toolRunMinimum("compact"));
    expect(compact).toHaveLength(1);
    if (compact[0]?.kind !== "tool-run") throw new Error("expected a work run");
    expect(compact[0].rows.map(row => row.key)).toEqual(["e1", "e2"]);
    // The diffs travel with the run, so its one line can still count the files
    // and their lines rather than saying "2 calls".
    expect(compact[0].diffs?.map(diff => diff.path)).toEqual(["/repo/a.ts", "/repo/b.ts"]);
  });

  it("groups adjacent MCP methods by server and preserves their order", () => {
    const row = (key: string, tool: string, result?: { ok: boolean; summary: string }):
      TranscriptRow => ({
      kind: "item", key, origin: "live",
      item: { kind: "tool", id: key, tool, summary: tool, input: {}, result },
    });
    const collapsed = collapseSuccessfulToolRuns([
      row("one", "node_repl/js", { ok: true, summary: "ok" }),
      row("two", "node_repl/screenshot", { ok: true, summary: "ok" }),
      row("three", "node_repl/js", { ok: true, summary: "ok" }),
      row("four", "slack/search", { ok: true, summary: "ok" }),
    ]);
    expect(collapsed).toHaveLength(1);
    expect(collapsed[0]).toMatchObject({ kind: "tool-run", tool: "work" });
    if (collapsed[0]?.kind !== "tool-run") throw new Error("expected a work run");
    expect(collapsed[0].rows.map(entry => entry.key)).toEqual(["one", "two", "three", "four"]);
  });

  it("keeps every transcript key the merged file-change row absorbed", () => {
    // A find hit lands on a KEY. The merged row now stands where a tool row and
    // its diff rows used to, so it has to answer to all of their keys or a hit
    // on the swallowed diff row scrolls to nothing.
    const collapsed = collapseSuccessfulToolRuns([
      { kind: "item", key: "t1", origin: "live", item: { kind: "tool", id: "t1",
        tool: "fileChange", summary: "update /repo/a.ts", input: {},
        result: { ok: true, summary: "ok" } } },
      { kind: "item", key: "d1", origin: "live",
        // The call that produced it. Merge pairs on this, never on adjacency.
        item: { kind: "diff", id: "d1", callId: "t1", path: "/repo/a.ts" } },
    ]);
    expect(collapsed).toHaveLength(1);
    expect(collapsed[0]).toMatchObject({ kind: "file-change", keys: ["t1", "d1"] });
  });

  it("folds adjacent bash and system calls into one command run", () => {
    const bash = (key: string, over: Partial<Extract<ConvItem, { kind: "tool" }>> = {}):
      TranscriptRow => ({
      kind: "item",
      key,
      origin: "live",
      item: {
        kind: "tool",
        id: key,
        tool: "Bash",
        summary: key,
        input: { command: key },
        result: { ok: true, summary: "done" },
        ...over,
      },
    });
    const one = collapseSuccessfulToolRuns([bash("ls")]);
    expect(one).toEqual([expect.objectContaining({
      kind: "tool-run", tool: "command",
    })]);
    if (one[0]?.kind !== "tool-run") throw new Error("expected a command run");
    expect(one[0].rows.map(row => row.key)).toEqual(["ls"]);

    const mixed = collapseSuccessfulToolRuns([
      bash("one"),
      bash("two", { tool: "execute" }),
      bash("three", { tool: "commandExecution" }),
    ]);
    expect(mixed).toHaveLength(1);
    if (mixed[0]?.kind !== "tool-run") throw new Error("expected a command run");
    expect(mixed[0].rows.map(row => row.key)).toEqual(["one", "two", "three"]);

    const split = collapseSuccessfulToolRuns([
      bash("before"),
      {
        kind: "item",
        key: "read",
        origin: "live",
        item: {
          kind: "tool",
          id: "read",
          tool: "Read",
          summary: "a.ts",
          input: {},
          result: { ok: true, summary: "ok" },
        },
      },
      bash("after"),
    ]);
    expect(split.map(entry => entry.kind)).toEqual(["tool-run"]);
    if (split[0]?.kind !== "tool-run") throw new Error("expected a work run");
    expect(split[0].tool).toBe("work");
    expect(split[0].rows.map(row => row.key)).toEqual(["before", "read", "after"]);
  });

  it("lays consecutive sibling subagents into one grid and leaves a lone spawn as a row", () => {
    const agent = (id: string): TranscriptRow => ({
      kind: "item", key: id, origin: "live",
      item: {
        kind: "subagent", id, subagentType: id, description: `${id} task`,
        status: "running", children: [], toolCount: 0, subagentCount: 0,
      },
    });
    const prose: TranscriptRow = {
      kind: "item", key: "t", origin: "live",
      item: { kind: "text", id: "t", text: "hi", streaming: false },
    };
    const grouped = groupSubagentGrids([
      { kind: "row", row: agent("a") },
      { kind: "row", row: agent("b") },
      { kind: "row", row: prose },
      { kind: "row", row: agent("c") },
    ]);
    expect(grouped.map(entry => entry.kind)).toEqual(["subagent-grid", "row", "row"]);
    if (grouped[0]?.kind !== "subagent-grid") throw new Error("expected a grid");
    expect(grouped[0].rows.map(row => row.key)).toEqual(["a", "b"]);
    expect(grouped[2]).toMatchObject({ kind: "row" });
  });

  it("draws the sibling grid and a running dock above the composer", () => {
    const agent = (id: string, status: "running" | "done"): ConvItem => ({
      kind: "subagent", id, subagentType: id, description: `${id} task`,
      status, children: [], toolCount: 0, subagentCount: 0,
    });
    const html = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      items: [agent("reviewer", "running"), agent("explorer", "done")],
    }));
    expect(html).toContain('class="sub-grid"');
    expect(html).toContain('class="sub-tile');
    expect(html).toContain("reviewer");
    expect(html).toContain("explorer");
    expect(html).toContain('class="conv-subrun"');
    expect(html).toContain('aria-label="reviewer running"');
    expect(html).not.toContain('aria-label="explorer running"');
  });

  it("says the count, and offers no control because the pane cannot page", () => {
    // Nothing here fetches an older window, so the seam is a sentence and only a
    // sentence: a button with no pager behind it would be a dead control.
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      history: { status: "ready", items: [], olderMessages: 853 },
    }));
    expect(markup).toContain("853 older messages are not shown.");
    expect(markup).not.toContain('class="btn conv-hist-more"');
    expect(markup).not.toContain(">Load older<");
  });

  it("drops the whole seam once nothing older remains", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      history: { status: "ready", items: [], olderMessages: 0 },
    }));
    expect(markup).not.toContain("older messages are not shown");
    expect(markup).not.toContain(">Load older<");
  });

  it("gives the reading column the pane width so prose does not wrap one word per line", () => {
    const markup = conversationCss;
    expect(markup).toMatch(/\.conv\s*\{[^}]*width:100%/);
    expect(markup).toMatch(/\.conv-column\s*\{[^}]*width:100%/);
    expect(markup).toMatch(/\.conv-column\s*\{[^}]*max-width:var\(--conv-measure\)/);
    expect(markup).toContain("overflow-wrap:break-word");
    expect(markup).not.toContain("word-break:break-word");
  });
});

describe("a folded run never hides a running or failed call", () => {
  const row = (key: string, tool: string, ok?: boolean): TranscriptRow => ({
    kind: "item", key, origin: "live",
    item: {
      kind: "tool", id: key, tool, summary: key, input: {},
      ...(ok === undefined ? {} : { result: { ok, summary: "" } }),
    },
  });

  it("leaves a failed call standing as its own row", () => {
    const collapsed = collapseSuccessfulToolRuns([
      row("r1", "Read", true), row("r2", "Read", true),
      row("bad", "Bash", false),
      row("r3", "Read", true), row("r4", "Read", true),
    ], 0);
    const failed = collapsed.find(entry =>
      entry.kind === "row" && entry.row.key === "bad");
    expect(failed).toBeDefined();
    for (const entry of collapsed) {
      if (entry.kind !== "tool-run") continue;
      expect(entry.rows.map(candidate => candidate.key)).not.toContain("bad");
    }
  });

  it("keeps a failed edit's diff with the failure, not with the next run", () => {
    // Every diff is foldable, so splitting a failed call out of the stretch
    // used to leave ITS diff at the head of the next partition: the following
    // run counted a file no call inside it touched, and drew "edited 2 files"
    // over a sentence with no failure anywhere in it.
    const patch = (key: string, ok: boolean): TranscriptRow => ({
      kind: "item", key, origin: "live",
      item: { kind: "tool", id: key, tool: "apply_patch", summary: `edit ${key}`,
        input: { file_path: `/repo/${key}.ts` }, result: { ok, summary: "" } },
    });
    const diff = (key: string, added: number, removed: number): TranscriptRow => ({
      kind: "item", key: `d-${key}`, origin: "live",
      item: { kind: "diff", id: `d-${key}`, callId: key, path: `/repo/${key}.ts`,
        added, removed },
    });
    const collapsed = collapseSuccessfulToolRuns([
      patch("e1", false), diff("e1", 8, 2),
      patch("e2", true), diff("e2", 3, 1),
    ], 0);
    // The refusal keeps the change it made, and mergeFileChangeRows still pairs
    // the two because they are still adjacent and still its own.
    expect(collapsed[0]).toMatchObject({ kind: "file-change", keys: ["e1", "d-e1"] });
    const run = collapsed.find(entry => entry.kind === "tool-run");
    if (run?.kind !== "tool-run") throw new Error("expected a work run");
    expect(run.rows.map(row => row.key)).toEqual(["e2"]);
    expect(run.diffs?.map(item => item.path)).toEqual(["/repo/e2.ts"]);
    // ONE file, and it is the one the run's own call changed.
    const label = workGroupPartsLabel(workGroupParts({
      tools: run.rows.map(row => row.item), diffs: run.diffs, running: false,
    }));
    expect(label).toContain("e2.ts");
    expect(label).not.toContain("e1");
    expect(label).not.toContain("2 files");
  });

  it("leaves an in-flight call standing as its own row", () => {
    const collapsed = collapseSuccessfulToolRuns([
      row("r1", "Read", true), row("live", "Bash"), row("r2", "Read", true),
    ], 0);
    for (const entry of collapsed) {
      if (entry.kind !== "tool-run") continue;
      expect(entry.rows.map(candidate => candidate.key)).not.toContain("live");
    }
  });
});

describe("Conversation — a tool row prints only what it has not already said", () => {
  const REPO = "/Users/ann/Projects/Interface";
  const paint = (over: Partial<ConversationProps>): string =>
    renderToStaticMarkup(createElement(Conversation, { ...baseProps, ...over }));

  it("prints a calm MCP name while keeping the exact raw id reachable", () => {
    // Codex returns toolName() as an mcpToolCall's summary, so the row read
    // "node_repl/js node_repl/js" — half of it a verbatim repeat.
    const html = paint({ items: [{
      kind: "tool", id: "m1", tool: "node_repl/js", summary: "node_repl/js",
      input: { code: "1+1" }, result: { ok: true, summary: "2" },
    }] });
    const row = html.slice(html.indexOf('class="conv-row conv-tool-row"'));
    expect(row).toContain(">JavaScript</span>");
    expect(row.split("node_repl/js").length - 1).toBe(2);   // aria-label + title
    expect(row).not.toContain('class="conv-sum"');
  });

  it("renders an MCP burst as activity lines that never swallow a failure", () => {
    const items = Array.from({ length: 24 }, (_, index): ConvItem => ({
      kind: "tool", id: `m${String(index)}`, tool: "node_repl/js",
      summary: "node_repl/js", input: { code: String(index) },
      result: { ok: index % 4 !== 0, summary: index % 4 === 0 ? "failed" : "ok" },
    }));
    const html = paint({ items });
    // Six of the 24 failed, so the burst is six activity lines of three and six
    // failed calls still standing on their own rows — never one line whose dim
    // summary is the only thing left of a call that blew up.
    expect(html).not.toContain("Called Node REPL 24 times");
    expect(html.match(/Called Node REPL 3 times\. Tools/gu)).toHaveLength(6);
    expect(html.match(/class="conv-row conv-tool-row"/gu)).toHaveLength(6);
    expect(html.match(/class="conv-state conv-inline conv-state-fail"/gu)).toHaveLength(6);
    expect(html).not.toContain(">node_repl/js</span>");
  });

  it("prints the first call's own result under a folded run", () => {
    // The ⎿ line in CLI chrome, and the whole reason a folded run is readable
    // without opening it. Refined chrome hides it with a rule rather than by
    // leaving the element out, so the markup carries it in both.
    const html = paint({ items: [
      { kind: "tool", id: "b1", tool: "Bash", summary: "npm test",
        input: { command: "npm test" },
        result: { ok: true, summary: "42 passed\n0 failed" } },
      { kind: "tool", id: "b2", tool: "Bash", summary: "npm run lint",
        input: { command: "npm run lint" }, result: { ok: true, summary: "clean" } },
    ] });
    expect(html).toContain("Ran 2 commands");
    // First line only: a run's line stays one line.
    expect(html).toContain('<span class="conv-sum conv-run-result">42 passed</span>');
    expect(html).not.toContain("0 failed");
    expect(conversationCss).toContain(".conv-run-result { display:none; }");
  });

  it("keeps a summary that merely begins with the tool's name", () => {
    const html = paint({ items: [{
      kind: "tool", id: "b1", tool: "Bash", summary: "bash -lc 'npm run build'",
      input: {}, result: { ok: true, summary: "ok" },
    }], pathRoot: REPO });
    expect(html).toContain("bash -lc &#x27;npm run build&#x27;");
  });

  it("shortens the absolute paths inside an ordinary summary", () => {
    const html = paint({ items: [{
      kind: "tool", id: "r1", tool: "Read", summary: `${REPO}/src/main/bridge.ts`,
      input: {}, result: { ok: true, summary: "ok" },
    }], pathRoot: REPO });
    expect(html).toContain(">src/main/bridge.ts<");
    // The absolute path is still reachable on the row and still indexed.
    expect(html).toContain(`title="${REPO}/src/main/bridge.ts"`);
  });

  it("counts the files instead of re-listing the ones its diff rows name", () => {
    // The engine slices this list at 120 chars with no ellipsis, so the row was
    // printing a path cut mid-word directly above the rows that name it whole.
    const paths = [
      `${REPO}/src/engines/claude/adapter.test.ts`,
      `${REPO}/src/main/bridge.test.ts`,
      `${REPO}/src/main/registry.test.ts`,
    ];
    const html = paint({
      pathRoot: REPO,
      items: [
        { kind: "tool", id: "fc", tool: "fileChange",
          summary: paths.map(p => `update ${p}`).join(", ").slice(0, 120),
          input: {}, result: { ok: true, summary: "3 files" } },
        ...paths.map((path, i) =>
          ({ kind: "diff" as const, id: `d${i}`, callId: "fc", path })),
      ],
    });
    expect(html).toContain(">3 files<");
    expect(html).not.toContain("/Users/ann/Deskt");
    expect(html).not.toContain(">src/main/bridge.test.ts<");
    // And it is now ONE row rather than four. The tool row and the three diff
    // rows under it were four renderings of one patch; the merged row carries
    // the count, the verb and the way into the diff, so the fold that used to
    // say "Edited 3 files" underneath has nothing left to fold.
    expect(html.match(/class="conv-row/gu)).toHaveLength(1);
    expect(html).not.toContain("Edited 3 files");
    // Two of the three kinds were cut off with the paths, so the verb is the
    // neutral one rather than a guess at "update".
    expect(html).toContain(">Changed<");
    // The whole original list stays on the row's title.
    expect(html).toContain("update /Users/ann/Projects/Interface/src/engines");
  });

  it("leaves a summary alone when no diff rows follow the call", () => {
    const html = paint({
      pathRoot: REPO,
      items: [{ kind: "tool", id: "fc", tool: "fileChange",
        summary: `update ${REPO}/a.ts, update ${REPO}/b.ts`,
        input: {}, result: { ok: true, summary: "2 files" } }],
    });
    expect(html).toContain("update a.ts, update b.ts");
    expect(html).not.toContain(">2 files<");
  });
});

describe("the access mode lives in the pane header, not the control row", () => {

  // The ONE exception, by the owner's 2026-10-06 Threads redesign: a pane in
  // the Threads grid draws its run settings as chips, access among them
  // (ThreadRunSettings). Every other pane's control row still has none.
  const threadSettings = composerSource.slice(
    composerSource.indexOf("function ThreadRunSettings("),
    composerSource.indexOf("function SendArrow("));
  const elsewhere = source.replace(threadSettings, "");

  it("no longer builds an access picker anywhere else in the composer", () => {
    expect(threadSettings).toContain("const showAccess");
    expect(elsewhere).not.toContain("conv-access-select");
    expect(elsewhere).not.toContain("const accessPicker");
    expect(elsewhere).not.toContain("const showAccess");
    expect(source).not.toContain("permissionSelectOptions");
    // A boundary rather than a prefix: `conv-session-model` is the OTHER class
    // on that line and stays, and it opens with these very characters.
    expect(source).not.toMatch(/conv-session-mode\b/u);
    // Nor does the pills row claim to be busy for it: a permission change in
    // flight is announced by the header selector's own `pending`, and marking
    // this region busy for a control it no longer contains says it twice. The
    // Threads row does contain one, so its busy state does count it.
    expect(elsewhere).not.toContain("config.pendingPermission");
    expect(threadSettings).toContain("config.pendingPermission === true");
  });
});

describe("Conversation — slash commands before the session is live", () => {
  const review: ProviderCommand = { name: "review", description: "Review changes" };
  const compact: ProviderCommand = { name: "compact", description: "Compact context" };

  /** A scope this pane could really discover, or a test that says why not. */
  const scopeOrThrow = (engine: string | undefined, cwd: string | undefined,
      model: string | undefined): ProviderCommandScope => {
    const scope = prelaunchCommandScope(engine, cwd, model);
    if (scope === null) throw new Error("expected a discoverable pre-launch scope");
    return scope;
  };

  /** The bridge echoes the validated scope back; a stub that does not is a lie. */
  const listsCommands = async (scope: ProviderCommandScope):
      Promise<ProviderCommandCatalogue> =>
    ({ scope, available: true, commands: [review, compact] });

  it("prefers the live catalogue over any pre-launch guess", () => {
    expect(effectiveProviderCommands([review], [compact])).toEqual([review]);
  });

  it("lets an invalidated session's known-empty list beat the fallback", () => {
    // `commands.invalidated` reports [], which MEANS "this session advertises
    // nothing". Reviving a pre-launch guess there would re-offer dead commands.
    expect(effectiveProviderCommands([], [review])).toEqual([]);
  });

  it("falls back only while the live catalogue is unknown", () => {
    expect(effectiveProviderCommands(undefined, [review])).toEqual([review]);
    expect(effectiveProviderCommands(undefined, null)).toEqual([]);
  });

  it("scopes discovery to the session's own directory and omits settingSources", () => {
    const scope = scopeOrThrow("claude", "/repo/scoped", "claude-opus-5");
    expect(scope).toEqual({ engine: "claude", cwd: "/repo/scoped", model: "claude-opus-5" });
    // OMITTED, never []: an empty list asks Claude to load no settings at all,
    // which is not what an unlaunched session would do.
    expect("settingSources" in scope).toBe(false);
    // The provider's own default model is a real scope; a missing cwd is not.
    expect(prelaunchCommandScope("claude", "/repo/scoped", undefined))
      .toEqual({ engine: "claude", cwd: "/repo/scoped" });
    expect(prelaunchCommandScope("claude", "/repo/scoped", "default"))
      .toEqual({ engine: "claude", cwd: "/repo/scoped" });
    expect(prelaunchCommandScope("claude", "", "claude-opus-5")).toBeNull();
    expect(prelaunchCommandScope(undefined, "/repo/scoped", "claude-opus-5")).toBeNull();
  });

  it("does not treat a cancelled sibling discovery as a permanent empty catalogue",
    async () => {
      const cwd = "/repo/superseded-retry";
      const scope = scopeOrThrow("claude", cwd, "claude-opus-5");
      let calls = 0;
      const load = async (asked: ProviderCommandScope):
          Promise<ProviderCommandCatalogue> => {
        calls += 1;
        if (calls === 1) {
          return { scope: asked, available: false, commands: [],
            reason: "A newer model or command scope was selected before this refresh started." };
        }
        return listsCommands(asked);
      };
      expect(await loadPrelaunchProviderCommands(scope, load)).toBeNull();
      expect(await loadPrelaunchProviderCommands(scope, load)).toEqual([review, compact]);
      expect(calls).toBe(2);
      expect(source).toContain("if (attempt >= 4)");
      expect(source).toContain("ask(attempt + 1)");
    });

  it("asks the provider once per scope however often the chat is re-opened", async () => {
    let calls = 0;
    const scope = scopeOrThrow("claude", "/repo/cached", "claude-opus-5");
    const load = async (asked: ProviderCommandScope): Promise<ProviderCommandCatalogue> => {
      calls += 1;
      return listsCommands(asked);
    };
    expect(await loadPrelaunchProviderCommands(scope, load)).toEqual([review, compact]);
    expect(await loadPrelaunchProviderCommands(scope, load)).toEqual([review, compact]);
    expect(calls).toBe(1);
  });

  it("offers a relaunched chat the same slash menu a live session would", async () => {
    const cwd = "/repo/relaunched";
    await loadPrelaunchProviderCommands(scopeOrThrow("claude", cwd, "claude-opus-5"),
      listsCommands);
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      drafts: drafted("/"),
      cwd,
      loadProviderCommands: listsCommands,
      config: { ...baseProps.config, engine: "claude", engineLabel: "Claude",
        model: "claude-opus-5" },
      onRunCommand: () => undefined,
    }));
    expect(markup).toContain('aria-label="Claude commands"');
    expect(markup).toContain('role="listbox"');
    expect(markup).toContain("/review");
    expect(markup).toContain("/compact");
  });

  it("shows the slash launcher on the pre-launch catalogue alone", async () => {
    // No `onRunCommand`, so the launcher appears only because commands exist —
    // and the unasked directory is the same pane with nothing discovered yet.
    const cwd = "/repo/launcher";
    await loadPrelaunchProviderCommands(scopeOrThrow("claude", cwd, "claude-opus-5"),
      listsCommands);
    const paneFor = (directory: string): string =>
      renderToStaticMarkup(createElement(Conversation, {
        ...baseProps,
        cwd: directory,
        loadProviderCommands: listsCommands,
        config: { ...baseProps.config, engine: "claude", engineLabel: "Claude",
          model: "claude-opus-5" },
      }));
    expect(paneFor(cwd)).toContain('aria-label="Claude commands"');
    expect(paneFor("/repo/never-asked")).not.toContain('aria-label="Claude commands"');
  });

  it("treats a provider that cannot list commands as no fallback at all", async () => {
    const cwd = "/repo/grok";
    const unavailable = async (scope: ProviderCommandScope):
        Promise<ProviderCommandCatalogue> =>
      ({ scope, available: false, commands: [], reason: "Grok cannot list commands." });
    expect(await loadPrelaunchProviderCommands(scopeOrThrow("acp:grok", cwd, undefined),
      unavailable)).toBeNull();
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      cwd,
      loadProviderCommands: unavailable,
      config: { ...baseProps.config, engine: "acp:grok", engineLabel: "Grok" },
    }));
    expect(markup).not.toContain('aria-label="Grok commands"');
  });

  it("keeps slash SENDING permissive when only the pre-launch list is known", () => {
    // A pre-launch catalogue may be incomplete, so it may widen the menu but
    // must never narrow the send gate from permissive to fail-closed.
    expect(source).toContain("const commandsKnown = props.commands !== undefined;");
    expect(source).toContain("const liveCommands = props.commands ?? NO_COMMANDS;");
    expect(source.match(/providerCommandIntent\(liveCommands, commandsKnown/gu))
      .toHaveLength(2);
    expect(source).not.toContain("providerCommandIntent(commands,");
    expect(source).toContain(
      "effectiveProviderCommands(props.commands, prelaunchCommands);");
  });
});

describe("Conversation — amber says one thing: an agent is blocked on you", () => {
  const css = conversationCss;

  it("has a CSS block to assert against", () => {
    expect(css).not.toBe("");
  });

  it("spends the attention hue on the approval card and nothing else", () => {
    // Every rule that reads --status-waiting, by selector. A find hit, a review
    // comment's left rule, its priority chip and the compacting counter each
    // used to take the hue, which made four amber marks in one pane mean four
    // different things and taught the user to ignore all of them.
    //
    // The third is the jump-latest capsule, and it is the SAME sentence said
    // where the card cannot be seen: the reader scrolled up, the approval is
    // off screen below them, and the agent is blocked until they answer it.
    // Its border borrows currentColor rather than naming the token twice.
    expect(css.match(/var\(--status-waiting\)/gu) ?? []).toHaveLength(3);
    expect(css).toContain(".conv-glyph-wait { color:var(--status-waiting); }");
    expect(css).toContain(".conv-approval-pending { border-color:var(--status-waiting); }");
    expect(css).toContain(".conv-jump-latest.is-waiting { color:var(--status-waiting);");
  });

  it("draws a find hit in the selection family", () => {
    expect(css).toContain("inset:0 auto 0 0; width:2px; background:var(--accent); }");
    // The current hit is the wider rail plus the wash and the outline — it no
    // longer needs its own background, because every hit is already accent.
    expect(css).toContain(".conv-find-current::before { width:3px; }");
  });

  it("keeps a review comment's rule and priority chip neutral", () => {
    expect(css).toMatch(
      /\.conv-md-code-comment \{[^{}]*border-left-width:3px;/u);
    expect(css).toMatch(/\.conv-md-priority \{[^{}]*color:var\(--fg-dim\);/u);
  });

  it("reads compaction as a process, not a summons", () => {
    expect(css).toContain(".conv-session-tokens.is-compacting { color:var(--fg-dim); }");
  });
});

describe("Conversation — the jump capsule says how much is behind the reader", () => {
  const css = conversationCss;

  it("labels the capsule from the pure backlog helpers", () => {
    // renderToStaticMarkup runs no effects and owns no scroller, so the capsule
    // — which exists only once a scroll event has reported the reader is away
    // from the bottom — is never in the painted markup. The wiring is the
    // assertion; what it computes is unit-tested in conversation.test.ts.
    expect(source).toContain("jumpLatestBacklog(rows, drawnKeys, jumpAnchor,");
    expect(source).toContain("jumpLatestLabel(jumpBacklog)");
    // The count is taken over what the pane DRAWS, so the dial cannot make the
    // capsule and the transcript disagree. `rows` stays the approval scan's
    // input, which is the one reading no fold may touch.
    expect(source).toContain("displayRows.map(displayRowKeys)");
    expect(source).toContain('className={jumpBacklog.approvalWaiting');
    expect(source).toContain('"conv-jump-latest is-waiting" : "conv-jump-latest"');
    expect(source).toContain("aria-label={jumpLabel.aria}");
    expect(source).toContain('<Icon name="chevron-down" sm />');
    expect(source).toContain('<span className="conv-jump-full">{jumpLabel.text}</span>');
    expect(source).toContain('<span className="conv-jump-short">{jumpLabel.short}</span>');
  });

  it("anchors the count once, on the edge into paused, and clears it on resume", () => {
    // ONE read of the ref feeds both setters. Reading `stick.current` twice
    // lets a concurrent render land between them and leave the capsule drawn
    // over a null anchor, or an anchor under a hidden capsule.
    expect(source).toContain("const following = stick.current;");
    // `current ?? lastRowKey` is the whole rule: re-anchoring on every scroll
    // event while still paused would zero the count each time the reader
    // nudged the wheel.
    expect(source).toContain(
      "setJumpAnchor(current => following ? null : current ?? lastRowKey)");
    expect(source).toContain("setAwayFromLatest(!following)");
    // Every way back to the latest drops the anchor: the button, the user's own
    // send, and a session switch that reopens at another transcript's end. Each
    // is pinned at ITS OWN site — the three share a line pair, so a bare
    // setAwayFromLatest/setJumpAnchor regex would pass on any one of them.
    expect(source).toMatch(/function jumpToLatest\(\)[\s\S]{0,220}setJumpAnchor\(null\)/u);
    expect(source).toMatch(
      /stick\.current = true;   \/\/ your own message[\s\S]{0,420}setJumpAnchor\(null\)/u);
    expect(source).toMatch(
      /if \(seenKey !== sessionKey\) \{[\s\S]{0,600}?setJumpAnchor\(null\);/u);
  });

  it("keeps a find jump visible to the capsule, so a later approval still lands", () => {
    // Search navigation breaks the pin by writing the ref directly. A match
    // already centred fires no scroll event, so onScroll never runs and the
    // capsule would stay hidden over a paused pane — and the brief's word for
    // the approval exception was "always".
    expect(source).toMatch(
      /stick\.current = false;[\s\S]{0,420}?setAwayFromLatest\(true\);[\s\S]{0,40}?node\.scrollIntoView/u);
  });

  it("keeps scroll-up a real pause: only a pin or a stuck reader moves the view", () => {
    // The one write that follows the stream. Turn completion sets no ref here
    // and runs no scroll of its own, so a finished turn leaves a paused reader
    // exactly where they were.
    expect(source).toContain("if (pinNext.current || stick.current) el.scrollTop = height;");
    expect(source).toContain(
      "stick.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 40;");
    expect(source.match(/el\.scrollTop = /gu) ?? []).toHaveLength(1);
    // History landing ABOVE the reader is the only other write, and it holds
    // the reader's line still rather than following anything.
    expect(source).toContain("el.scrollTop += grew;");
  });

  it("jumps instantly for a reader who asked for less motion", () => {
    expect(source).toContain("behavior: jumpScrollBehavior(");
    expect(source).not.toContain('behavior: "smooth"');
  });

  it("shortens the label at narrow widths instead of wrapping it", () => {
    expect(css).toContain(".conv-jump-short { display:none; }");
    expect(css).toMatch(
      /@container \(max-width: 420px\) \{[\s\S]*?\.conv-jump-full \{ display:none; \}/u);
    expect(css).toMatch(
      /@container \(max-width: 420px\) \{[\s\S]*?\.conv-jump-short \{ display:block; \}/u);
    expect(css).toMatch(/\.conv-jump-full, \.conv-jump-short \{[^{}]*white-space:nowrap;/u);
    expect(css).toMatch(/\.conv-jump-full, \.conv-jump-short \{[^{}]*text-overflow:ellipsis;/u);
  });

  it("hands the backlog every transcript key each drawn line stands for", () => {
    const bash = (index: number): TranscriptRow => ({
      kind: "item", key: `l:tool-${String(index)}`, origin: "live",
      item: { kind: "tool", id: `tool-${String(index)}`, tool: "Bash",
        summary: `npm run check ${String(index)}`, input: {},
        result: { ok: true, summary: "done" } },
    });
    const rows: TranscriptRow[] = [
      { kind: "item", key: "l:text-0", origin: "live",
        item: { kind: "text", id: "text-0", text: "on it", streaming: false } },
      ...Array.from({ length: 12 }, (_unused, index) => bash(index)),
    ];
    // THE REVIEWER'S CASE, END TO END, at every step of the dial: twelve Bash
    // calls behind a paused reader are ONE folded line, so the capsule must say
    // "1 new" and never "12 new".
    for (const density of ["compact", "balanced", "detailed"] as const) {
      const display = groupSubagentGrids(
        collapseSuccessfulToolRuns(rows, toolRunMinimum(density)));
      const drawn = display.map(displayRowKeys);
      expect(drawn.length, density).toBe(2);
      expect(drawn[0], density).toEqual(["l:text-0"]);
      expect(drawn[1], density).toEqual(rows.slice(1).map(row => row.key));
      expect(jumpLatestBacklog(rows, drawn, "l:text-0").count, density).toBe(1);
    }
  });

  it("keeps the capsule in the overlay idiom the rest of the pane uses", () => {
    expect(css).toMatch(/\.conv-jump-latest \{[^{}]*background:var\(--surface-raised\);/u);
    expect(css).toMatch(
      /\.conv-jump-latest \{[^{}]*border:var\(--border-w\) solid var\(--border-strong\);/u);
    expect(css).toMatch(/\.conv-jump-latest \{[^{}]*border-radius:var\(--radius-pill\);/u);
    expect(css).toMatch(/\.conv-jump-latest \{[^{}]*box-shadow:var\(--overlay-shadow\);/u);
    expect(css).toMatch(/\.conv-jump-latest \{[^{}]*font-size:var\(--text-ui\);/u);
    // A count that renumbers character by character as the stream lands.
    expect(css).toMatch(/\.conv-jump-latest \{[^{}]*font-variant-numeric:tabular-nums;/u);
  });
});

describe("Conversation — the prompt this region is the answer to", () => {
  const css = conversationCss;
  const anchorCss = css.slice(css.indexOf(".conv-anchor {"),
    css.indexOf(".conv-anchor-step:disabled"));

  it("reads the anchor from the pure helpers rather than restating the rule", () => {
    expect(source).toMatch(
      /import \{[\s\S]*?promptAnchorKey,[\s\S]*?\} from "\.\.\/transcript-navigation"/u);
    expect(source).toContain("promptAnchorView(prompts, anchorKey)");
    expect(source).toContain("useMemo(() => transcriptPrompts(rows), [rows])");
    expect(source).toContain("promptAnchorKey(spots, el.clientHeight)");
  });

  it("measures from the pane's ONE scroll handler, never a second listener", () => {
    // Task 7's handler already owns stick/awayFromLatest/jumpAnchor for this
    // scroller. A second listener would read the same event twice and could
    // answer from a different frame than the capsule beside it.
    expect(source).toMatch(
      /function onScroll\(\) \{[\s\S]*?measurePromptAnchor\(\);[\s\S]*?\n  \}/u);
    expect(source.match(/onScroll=\{onScroll\}/gu) ?? []).toHaveLength(1);
    expect(source).not.toContain('addEventListener("scroll"');
  });

  it("draws one truncating line: glyph, prompt, ordinal, both steppers", () => {
    expect(source).toContain('className="conv-anchor"');
    // `comment`, not `play`: play already means Steer and the queue's Send now,
    // and this bar points back at a prompt rather than forward at an act.
    expect(source).toContain('<Icon name="comment" className="conv-anchor-glyph" sm />');
    // The full prompt is the tooltip, because the line is one line.
    expect(source).toMatch(
      /className="conv-anchor-text" title=\{anchor\.text\}/u);
    expect(source).toMatch(/\{anchor\.ordinal\}\/\{anchor\.total\}/u);
    expect(source).toMatch(/aria-label="Previous prompt"/u);
    expect(source).toMatch(/aria-label="Next prompt"/u);
    // Both ends stop rather than wrapping, and say so to the pointer and to AT.
    expect(source).toContain("disabled={anchor.prevKey === null}");
    expect(source).toContain("disabled={anchor.nextKey === null}");
    // Clicking the line goes back to the prompt it names.
    expect(source).toMatch(/onClick=\{\(\) => \{ goToPrompt\(anchor\.key\); \}\}/u);
  });

  it("hides itself the moment the newest prompt is on screen", () => {
    // The helper answers null there; the bar is gated on that one value, so
    // there is no second rule in the markup that could disagree with it.
    expect(source).toMatch(/\{anchor !== null && \([\s\S]{0,80}?className="conv-anchor"/u);
  });

  it("stacks above the scroller, so it and the sticky plan header cannot fight", () => {
    // DECIDED, not accidental: the bar is a rail ABOVE .conv-list rather than a
    // second sticky at top:0 inside it. The plan header keeps its own sticky
    // top:0 and slides under this bar's bottom edge; neither has to know the
    // other's height and there is no z-index contest.
    expect(source).toContain('className="conv-anchor"');
    expect(source.indexOf('className="conv-anchor"'))
      .toBeLessThan(source.indexOf('className="conv-list"'));
    expect(anchorCss).toContain(".conv-anchor {");
    expect(anchorCss).not.toContain("position:sticky");
    expect(anchorCss).not.toContain("z-index");
    expect(css).toMatch(/\.conv-plan \{[\s\S]*?position:sticky; z-index:2; top:0;/u);
  });

  it("never drops the keyboard on the one step that unmounts the bar", () => {
    // Landing the NEWEST prompt at the top is the state promptAnchorKey answers
    // null for, so the button being pressed is removed a few frames into the
    // scroll — and a focused element that is removed drops focus to
    // document.body. Focus goes to the transcript FIRST: it is what the step
    // was for, and role="log" + aria-label already name it.
    expect(source).toContain(
      "if (key === prompts[prompts.length - 1]?.key) el.focus({ preventScroll: true });");
    // el.focus() is a no-op on an element that cannot hold focus, so the log
    // container carries tabIndex=-1 — focusable by script, still out of the
    // tab order.
    expect(source).toMatch(
      /className="conv-list" ref=\{listRef\} onScroll=\{onScroll\} tabIndex=\{-1\}[\s\S]{0,120}?role="log"/u);
    // Before the scroll, not after it: the scroll is smooth by default and the
    // unmount lands somewhere in the middle of it.
    const step = source.indexOf("function goToPrompt");
    expect(source.indexOf("el.focus({ preventScroll: true })", step))
      .toBeLessThan(source.indexOf("el.scrollTo({", step));
  });

  it("says the ordinal as a sentence for a reader who cannot see the slash", () => {
    expect(source).toContain(
      "aria-label={`Prompt ${anchor.ordinal} of ${anchor.total}`}");
  });

  it("steps instantly for a reader who asked for less motion", () => {
    // The same scrollTo route the jump capsule takes, so the one animation a
    // stylesheet cannot reach reads the preference in exactly one place.
    expect(source).toMatch(
      /function goToPrompt\([\s\S]*?el\.scrollTo\(\{[\s\S]*?behavior: jumpScrollBehavior\(/u);
  });

  it("clears the anchor on every route back to the newest prompt", () => {
    expect(source).toMatch(
      /if \(seenKey !== sessionKey\) \{[\s\S]{0,700}?setAnchorKey\(null\);/u);
    expect(source).toMatch(/\/\/ your own message[\s\S]{0,900}?setAnchorKey\(null\);/u);
  });

  it("frosts the bar from the material tokens, and names no colour of its own", () => {
    // CLI chrome, reduced transparency and the flat palettes all collapse these
    // tokens in theme.css, so the bar flattens to a plain text line with the
    // rest of the app instead of keeping a private blur.
    expect(anchorCss).toContain("background:var(--material-chrome)");
    expect(anchorCss).toContain(
      "backdrop-filter:blur(var(--material-blur)) saturate(var(--material-saturate))");
    expect(anchorCss).toMatch(/\.conv-anchor-text \{[^{}]*color:var\(--fg-muted\);/u);
    // --fg-dim, and NOT --fg-faint: faint is the non-text tier (theme.css:81-86,
    // and the same rule restated at Conversation.tsx, Composer.tsx and
    // DiffView.tsx) — it measures 2.12:1 on --chrome-bg, and "3/7" is text.
    expect(anchorCss).toMatch(/\.conv-anchor-ord \{[^{}]*color:var\(--fg-dim\);/u);
    expect(anchorCss).not.toMatch(/color:var\(--fg-faint\)/u);
    expect(anchorCss).toMatch(/\.conv-anchor-ord \{[^{}]*font-family:var\(--font-mono\);/u);
    // The type scale's own names, not loose pixels beside them.
    expect(anchorCss).toMatch(/\.conv-anchor-ord \{[^{}]*font-size:var\(--text-meta\);/u);
    expect(anchorCss).toMatch(/\.conv-anchor-text \{[^{}]*font-size:var\(--text-ui\);/u);
    // One strip's height, the same as every other strip in the window.
    expect(anchorCss).toMatch(/\.conv-anchor \{[^{}]*min-height:var\(--strip-h\);/u);
    expect(anchorCss).not.toContain("26px");
    expect(anchorCss).not.toMatch(/#[0-9a-f]{3,8}\b/iu);
  });
});

describe("Conversation — the caller's slot above the composer", () => {

  it("renders the slot directly above the composer, on the reading measure", () => {
    expect(source).toMatch(/beforeComposer\?: ReactNode;/u);
    // Inside the composer strip, above everything the strip already draws — so
    // it lands on the same measure the composer and the dirty pill use.
    const strip = source.indexOf('<div className="conv-strip">');
    expect(strip).toBeGreaterThan(0);
    expect(source.indexOf("{props.beforeComposer}")).toBeGreaterThan(strip);
    expect(source.indexOf("{props.beforeComposer}"))
      .toBeLessThan(source.indexOf('<div className="conv-composer">'));
  });

  it("renders a pane that was given no slot exactly as it always did", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, baseProps));
    expect(markup).toContain("conv-strip");
    expect(markup).not.toContain("tile-recap");
  });
});

describe("Conversation — the approval buttons print their keys", () => {
  const css = conversationCss;

  const blocked = {
    ...baseProps,
    live: true,
    items: [{
      kind: "approval", id: "ap-1", tool: "Bash", summary: "npm run check", input: {},
    }],
    activeApprovalIds: new Set(["ap-1"]),
  } as ConversationProps;

  it("keeps the approval head's two voices apart", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...blocked,
      config: { ...baseProps.config, engineLabel: "Claude Code" },
    } as ConversationProps));
    // This suite runs in node, where useDocumentChrome finds no document and
    // answers "refined" — so this markup IS the refined branch.
    expect(markup).toContain('<span class="conv-name">Approval needed</span>');
    expect(markup).toContain("Bash — npm run check");
    expect(markup).not.toContain("wants to run");
    // The CLI branch is pinned where it can be: the source. The name is the
    // pane's own engine label, and an empty one falls back to the asking tool.
    expect(source).toContain('<span className="conv-name">{asking} wants to run</span>');
    expect(source).toContain('const asking = (engineLabel ?? "").trim() || item.tool;');
    expect(source).toContain("keyed={keyed} engineLabel={all.config.engineLabel}");
  });

  const asked = {
    ...baseProps,
    live: true,
    items: [{
      kind: "approval", id: "q1", tool: "AskUserQuestion", summary: "AskUserQuestion",
      input: {},
      questions: [{
        question: "Which library?", header: "Library", multiSelect: false,
        options: [{ label: "date-fns", description: "small" }],
      }],
    }],
    activeApprovalIds: new Set(["q1"]),
  } as ConversationProps;

  it("prints the chip on Allow and Deny, and announces the same binding", () => {
    const markup = renderToStaticMarkup(createElement(Conversation,
      { ...blocked, focused: true }));
    expect(markup).toContain('data-approval-key="allow"');
    expect(markup).toContain('aria-keyshortcuts="Meta+Enter Control+Enter"');
    expect(markup).toContain('data-approval-key="deny"');
    expect(markup).toContain('aria-keyshortcuts="Meta+Backspace Control+Backspace"');
    expect(markup).toContain('Allow<span class="kbd-hint" aria-hidden="true">⌘↩</span>');
    expect(markup).toContain('Deny<span class="kbd-hint" aria-hidden="true">⌘⌫</span>');
  });

  it("prints them on the question card too, on the same two verdicts", () => {
    const markup = renderToStaticMarkup(createElement(Conversation,
      { ...asked, focused: true }));
    expect(markup).toContain(
      'Send answers<span class="kbd-hint" aria-hidden="true">⌘↩</span>');
    expect(markup).toContain('Skip<span class="kbd-hint" aria-hidden="true">⌘⌫</span>');
    // The unanswered card's primary is disabled, so the chord it advertises is
    // refused by the same attribute a click is: there is no second path.
    expect(markup).toMatch(/disabled=""[^>]*data-approval-key="allow"/u);
  });

  /* Four tiles can be blocked at once and the chord answers exactly one of
     them. A chip on the pane that would NOT receive it is a printed lie that
     ends with the wrong command allowed. */
  it("says nothing about keys on a pane that does not own them", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, blocked));
    expect(markup).toContain("Allow</button>");
    // (the CSS block travels in the same markup, so the chip is looked for as a
    // rendered class rather than as the string)
    expect(markup).not.toContain('class="kbd-hint"');
    expect(markup).not.toContain("data-approval-key");
    expect(markup).not.toContain('aria-keyshortcuts="Meta+Enter');
  });

  /* TWO OUTSTANDING REQUESTS, ONE CHIP. The pane's listener clicks the LAST
     data-approval-key button under its root, so printing the key on both cards
     would mean pressing it while reading the upper card allows the LOWER card's
     command — and a disabled Allow at the bottom would leave the chord dead
     under a chip promising it works. The tie-break is made visible instead. */
  it("prints the key on the card that will actually receive it, and no other", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, {
      ...baseProps,
      live: true,
      focused: true,
      items: [
        { kind: "approval", id: "ap-1", tool: "Bash", summary: "the upper command", input: {} },
        { kind: "approval", id: "ap-2", tool: "Bash", summary: "the lower command", input: {} },
      ],
      activeApprovalIds: new Set(["ap-1", "ap-2"]),
    } as ConversationProps));
    // Both cards are pending and both offer their buttons…
    expect(markup.match(/class="conv-approval conv-approval-pending"/gu) ?? [])
      .toHaveLength(2);
    // …and exactly one of them says anything about a key.
    expect(markup.match(/data-approval-key="allow"/gu) ?? []).toHaveLength(1);
    expect(markup.match(/data-approval-key="deny"/gu) ?? []).toHaveLength(1);
    expect(markup.match(/class="kbd-hint"/gu) ?? []).toHaveLength(2);
    // It is the lower card — the one the listener's last-match rule reaches.
    expect(markup.indexOf('data-approval-key="allow"'))
      .toBeGreaterThan(markup.indexOf("the lower command"));
  });

  /* The chord CLICKS the button. Not a second verdict path beside the click
     one: the same handler, the same optimistic latch, the same focus settle,
     and Deny still opening its reason field first. */
  it("routes the chord through the button rather than a verdict of its own", () => {
    expect(source).toContain("button.click();");
    expect(source).toContain("if (button === undefined || button.disabled) return;");
    expect(source).toContain(
      "approvalKeyTyping(target instanceof HTMLElement ? target : null)");
    // Pane-scoped exactly like Find: the focused tile only, focus inside it or
    // nowhere, and never a defaultPrevented event.
    const listener = source.slice(source.indexOf("const onApprovalKey"));
    expect(listener.slice(0, 400)).toContain("if (event.defaultPrevented) return;");
    expect(listener).toContain("paneOwnsShortcut(root, event)");
    // …which is the Find listener's own scoping rule, shared rather than restated.
    expect(source.slice(source.indexOf("function paneOwnsShortcut"), 900
      + source.indexOf("function paneOwnsShortcut")))
      .toContain("conversationOwnsFindShortcut(");
    expect(source).toContain("window.addEventListener(\"keydown\", onApprovalKey);");
    expect(source).toContain("if (props.focused !== true) return;");
  });

  /* AN OFF-SCREEN CARD COSTS TWO PRESSES. The first scrolls it into view and
     answers nothing; the second, with the request on screen, answers it. The
     ordering below is the whole guarantee: the reveal branch returns BEFORE
     the click, so there is no path on which a hidden card is answered. */
  it("shows a card the reader cannot see instead of answering it", () => {
    const listener = source.slice(source.indexOf("const onApprovalKey"),
      source.indexOf("window.addEventListener(\"keydown\", onApprovalKey);"));
    expect(listener).toContain('const card = button.closest(".conv-approval, .thread-dock-card");');
    // Measured against the SCROLLER, which is the box the reader sees through —
    // for a card inside it. A Threads pane's docked card sits outside it, above
    // the composer, always on screen, and is answered at once.
    expect(listener).toContain("if (list !== null && card !== null && list.contains(card)) {");
    expect(listener).toContain("const region = list.getBoundingClientRect().top;");
    expect(listener).toContain("list.clientHeight) === \"reveal\"");
    const reveal = listener.indexOf("approvalChordStep(");
    const scroll = listener.indexOf("card.scrollIntoView({");
    const bail = listener.indexOf("return;", scroll);
    const click = listener.indexOf("button.click();");
    expect(reveal).toBeGreaterThan(0);
    expect(scroll).toBeGreaterThan(reveal);
    expect(bail).toBeGreaterThan(scroll);
    expect(click).toBeGreaterThan(bail);
    // Reduced motion is honoured on the same route the capsule and the prompt
    // steppers take — a script-paced scroll is invisible to the stylesheet.
    expect(listener).toContain("behavior: jumpScrollBehavior(");
  });

  it("draws the chip at the type floor, in tokens, on both button tiers", () => {
    expect(css).toContain(".kbd-hint { display:inline-flex; align-items:center; padding:0 4px;");
    expect(css).toContain(
      "font-family:var(--font-mono); font-size:var(--text-meta); line-height:var(--lh-prose);");
    expect(css).toContain("color:var(--fg-dim); border:var(--border-w) solid var(--border);");
    expect(css).toContain(".btn-primary .kbd-hint { color:var(--accent-fg); opacity:.9;");
    expect(css).toContain(
      "border-color:color-mix(in srgb, var(--accent-fg) 40%, transparent); }");
    const chip = css.slice(css.indexOf(".kbd-hint"));
    expect(chip.slice(0, 500)).not.toMatch(/#[0-9a-f]{3,8}\b/iu);
    // The chip reads its size off the ramp, so nothing here can sit below the
    // --text-meta floor it used to spell as a literal 10px.
    expect(chip.slice(0, 500)).not.toMatch(/font-size:\s*\d+px/u);
  });
});

describe("Conversation — an agent approval the policy is still checking", () => {
  const approval: ConvItem = {
    kind: "approval", id: "ap-1", tool: "Bash", summary: "npm run check", input: {},
  };
  const question: ConvItem = {
    kind: "approval", id: "q-1", tool: "AskUserQuestion", summary: "AskUserQuestion", input: {},
    questions: [{ question: "Which library?", header: "Library", multiSelect: false,
      options: [{ label: "date-fns", description: "small" }] }],
  };
  const render = (item: ConvItem, gate?: ConversationProps["approvalGate"]): string =>
    renderToStaticMarkup(createElement(Conversation, {
      ...baseProps, live: true, focused: true, items: [item],
      activeApprovalIds: new Set([item.id]),
      ...(gate === undefined ? {} : { approvalGate: gate }),
    } as ConversationProps));

  it("shows the request with no Allow, Deny or chord while Interface decides", () => {
    for (const item of [approval, question]) {
      const markup = render(item, id => id === item.id ? "checking" : null);
      expect(markup).toContain("Checking with Interface&#x27;s approval policy…");
      expect(markup).not.toContain('class="conv-acts"');
      expect(markup).not.toContain("data-approval-key");
      expect(markup).not.toContain('class="kbd-hint"');
      expect(markup).not.toContain('role="alert"');
    }
    expect(render(approval, () => "checking")).toContain("npm run check");
  });

  it("keeps today's card when the owner is asked or the session has no agent gate", () => {
    for (const gate of [() => "owner" as const, () => null, undefined]) {
      const markup = render(approval, gate);
      expect(markup).toContain('data-approval-key="allow"');
      expect(markup).not.toContain("Checking with Interface");
    }
  });
});

describe("Conversation — what floats over the transcript is anchored to it", () => {
  const css = conversationCss;

  /* THE CAPSULE USED TO COVER THE RECAP'S DISMISS BUTTON. It was positioned
     against .conv, whose box runs past the status sink, the composer and the
     caller's slot, so "above all of that" was arithmetic (bottom:108px) that
     held only at the heights those three happened to have. Anchored to the
     scroller, it is a fact instead. */
  it("gives the scroller its own positioning context", () => {
    expect(css).toContain(".conv-scroll { position:relative; z-index:1; display:flex;"
      + " flex-direction:column;\n  flex:1; min-width:0; min-height:0; }");
    // BOTH hermes plates stay under the text. .conv::before and .conv::after
    // are z-index:0, and a positioned sibling at z-index:auto sorts BETWEEN
    // them by tree order — which put one plate over the transcript and one
    // under it, swapping every 12s. Everything that must still cover the
    // transcript clears the new context from outside it: the drop overlay (6)
    // is a sibling of this wrapper, and the composer's popovers sit in the
    // non-positioned .conv-strip, so both outrank it. Everything INSIDE it —
    // the sticky plan header (2), a find hit (2), the jump capsule (4) — only
    // ever needs to be above transcript rows, which it still is.
    const dropCss = readFileSync(new URL("./DropIndicator.css", import.meta.url), "utf8");
    expect(dropCss).toMatch(/\.drop-indicator \{[^{}]*z-index:\s*6;/u);
    expect(css).toMatch(/\.conv-context-meter-panel \{[^{}]*z-index:6;/u);
    expect(css).toMatch(/\.conv-strip \{ flex:none; padding:0;/u);
    // The scroller keeps the flex behaviour it had as a direct child of .conv —
    // it grows into the wrapper and is still allowed to shrink below content.
    expect(css).toMatch(/\.conv-list \{[^{}]*flex:1;/u);
    expect(css).toMatch(/\.conv-list \{[^{}]*min-height:0;/u);
    expect(css).toMatch(/\.conv-list \{[^{}]*overflow-y:auto;/u);
  });

  it("floats the capsule 12px above the transcript, not 108px above the pane", () => {
    expect(css).toMatch(/\.conv-jump-latest \{[^{}]*bottom:12px;/u);
    // Scoped to the declaration: the comment above the rule still NAMES the old
    // offset, because why it was wrong is the useful half of the record.
    expect(css).not.toMatch(/\.conv-jump-latest \{[^{}]*bottom:108px/u);
  });

  it("wraps the scroller and the capsule together, in that order", () => {
    const wrapper = source.indexOf('<div className="conv-scroll">');
    const list = source.indexOf('<div className="conv-list" ref={listRef}');
    const capsule = source.indexOf('className={jumpBacklog.approvalWaiting');
    const close = source.indexOf("</div>{/* /.conv-scroll */}");
    expect(wrapper).toBeGreaterThan(0);
    expect(list).toBeGreaterThan(wrapper);
    expect(capsule).toBeGreaterThan(list);
    expect(close).toBeGreaterThan(capsule);
    // The wrapper is markup only: the scroller it wraps is still the element
    // the ref, the scroll handler and the log role live on, so nothing about
    // measuring, sticking or announcing moved with it.
    expect(source).toContain('<div className="conv-list" ref={listRef}'
      + " onScroll={onScroll} tabIndex={-1}");
    expect(source).toMatch(/<div className="conv-list"[^>]*\n\s*role="log"/u);
  });

  it("renders the wrapper around a real transcript", () => {
    const markup = renderToStaticMarkup(createElement(Conversation, baseProps));
    expect(markup).toContain('<div class="conv-scroll"><div class="conv-list"');
  });
});

describe("Conversation — the prompt anchor measures what it has to", () => {
  const body = source.slice(source.indexOf("function measurePromptAnchor()"),
    source.indexOf("function goToPrompt("));

  /* One rect per prompt per scroll event was O(prompts) on the hottest path in
     the app. Tops rise with document order, so the first prompt at or above the
     region's top edge — walking BACK from the newest — is the last one
     promptAnchorKey would find walking forward, and nothing above it can change
     the answer. Same answer, one to three rects. */
  it("walks back from the newest and stops at the first prompt off the top", () => {
    expect(body).toContain("for (let index = prompts.length - 1; index >= 0; index -= 1)");
    expect(body).toContain("spots.unshift({ key: prompt.key, top });");
    expect(body).toContain("if (top <= 0) break;");
    // The RULE did not move: the collected tail still goes to the same pure
    // function, which still sees the newest spot last.
    expect(body).toContain("promptAnchorKey(spots, el.clientHeight)");
    // And it is still the scroll handler's one caller.
    expect(source).toMatch(/function onScroll\(\) \{[\s\S]*?measurePromptAnchor\(\);[\s\S]*?\n  \}/u);
  });
});

describe("conversationEscapeAction", () => {
  const event = (key: string) => ({ key } as never);

  it("stops a running turn", () => {
    expect(conversationEscapeAction(event("Escape"),
      { thinking: true, commandMenuOpen: false })).toBe("stop");
  });

  it("does nothing when there is no turn to stop", () => {
    expect(conversationEscapeAction(event("Escape"),
      { thinking: false, commandMenuOpen: false })).toBe("pass");
  });

  it("yields to the slash-command menu, which closes on the same key", () => {
    expect(conversationEscapeAction(event("Escape"),
      { thinking: true, commandMenuOpen: true })).toBe("pass");
  });

  it("claims no other key", () => {
    for (const key of ["Enter", "Tab", "a", "Backspace"]) {
      expect(conversationEscapeAction(event(key),
        { thinking: true, commandMenuOpen: false }), key).toBe("pass");
    }
  });

  it("advertises esc only where the key can actually reach the handler", () => {
    // A disabled textarea fires no keydown, so a locked composer that still
    // printed `esc interrupt` would be promising a key that does nothing —
    // the exact failure sessionLine2's running gate exists to prevent.
    expect(source).toContain("running={thinking && !composerLocked}");
    // A Threads pane's empty field also answers a docked request, and only
    // while it is not locked (threadComposerAnswersApproval refuses a locked
    // field; thread-composer.test.ts holds the rule).
    expect(source).toContain(': thinking && !composerLocked ? "Escape" : undefined}');
    expect(source).toContain(
      "threadComposerAnswersApproval({ waiting: approvalPending, draft, locked: composerLocked })");
  });
});

describe("Conversation — the transcript row keeps the field the reader scans", () => {
  const css = conversationCss;
  const shrinkOf = (selector: string): number => {
    const at = css.indexOf(`\n${selector} {`);
    expect(at).toBeGreaterThan(-1);
    const block = css.slice(at, css.indexOf("}", at));
    const shorthand = block.match(/flex:\s*[\d.]+\s+([\d.]+)/u);
    if (shorthand !== null) return Number(shorthand[1]);
    const single = block.match(/flex-shrink:\s*([\d.]+)/u);
    return single === null ? 1 : Number(single[1]);
  };

  it("makes a tool row's path yield before its method name", () => {
    // Equal priority is weighted by content basis, so "Read" beside a long path
    // lost the same PERCENTAGE the path did and rendered "R…".
    expect(shrinkOf(".conv-tool-row .conv-name"))
      .toBeLessThan(shrinkOf(".conv-tool-row .conv-sum"));
    // Not zero: flex:none on this slot pushes a 60-character MCP row's run
    // state off the row entirely, which is what the rule above it records.
    expect(shrinkOf(".conv-tool-row .conv-name")).toBeGreaterThan(0);
  });
});

describe("Conversation — the small print of the composer row", () => {
  const css = conversationCss;

  it("keeps the steer pill on the radius token the form axis can square", () => {
    // .conv-send-queue's literal 12px is on theme.css's escape list and is
    // zeroed under CLI chrome; this one was not, so it alone stayed rounded.
    expect(css).toMatch(/\.conv-steer \{[^{}]*border-radius:var\(--radius-pill\);/u);
  });

  it("styles the command-launcher glyph from the stylesheet, not from JSX", () => {
    expect(source).toContain('<span aria-hidden="true" className="conv-slash-glyph">/</span>');
    expect(css).toContain(".conv-slash-glyph { font-family:var(--font-mono);");
    expect(source).not.toContain('style={{ fontFamily: "var(--font-mono)"');
  });

  it("gives the labelled composer status line a role its name can attach to", () => {
    // role=generic takes no accessible name, so the label was dropped entirely.
    // Asserted on the source: this wrapper belongs to the CLI chrome, and the
    // no-DOM harness has no <html data-chrome> to reach that branch with.
    expect(source).toMatch(
      /role="group"\s*aria-label=\{sessionStatusBarLabel\(status\)\}/u);
  });
});

describe("Conversation — the transcript's per-row boundary", () => {
  const css = conversationCss;

  it("ignores unrelated row state but observes live, find, and approval changes", () => {
    const item: ConvItem = { kind: "text", id: "text", text: "settled", streaming: false };
    const row = {
      item, findKey: "l:text", density: "balanced" as const, densityEpoch: 0,
      find: { active: false, matches: new Set<string>(), currentKey: null, register: () => undefined },
      all: { ...baseProps, keyedApprovalId: null, checkingApprovalIds: new Set<string>() },
    };
    expect(sameConversationItemProps(row, {
      ...row, all: { ...row.all, activeApprovalIds: new Set(["other"]), focused: true },
    })).toBe(true);
    expect(sameConversationItemProps(row, { ...row, all: { ...row.all, live: false } })).toBe(false);
    expect(sameConversationItemProps(row, { ...row, find: { ...row.find, active: true, matches: new Set(["l:text"]) } })).toBe(false);
    const approval = { ...row, item: { kind: "approval" as const, id: "a", tool: "Bash", summary: "Run", input: {} } };
    expect(sameConversationItemProps(approval, { ...approval, all: { ...approval.all, activeApprovalIds: new Set(["a"]) } })).toBe(false);
    expect(sameConversationItemProps(approval, { ...approval, all: { ...approval.all, onApprove: () => undefined } })).toBe(false);
    expect(sameConversationItemProps(approval, { ...approval, all: { ...approval.all, checkingApprovalIds: new Set(["a"]) } })).toBe(false);
  });

  it("answers a settled row's identical props without reading inside them", () => {
    // What a streamed token hands every row it did not touch: the same item,
    // context and find objects. Reading any of them would throw here.
    const untouchable = <T extends object>(): T => new Proxy({} as T, {
      get() { throw new Error("read"); }, ownKeys() { throw new Error("read"); },
    });
    const row = { item: untouchable<ConvItem>(), findKey: "l:text", density: "balanced" as const,
      densityEpoch: 0, find: untouchable<FindRenderContext>(), all: untouchable<ConversationItemContext>() };
    expect(sameConversationItemProps(row, { ...row })).toBe(true);
    expect(sameConversationItemProps(row, { ...row, densityEpoch: 1 })).toBe(false);
    const run = { run: {}, all: untouchable<ConversationItemContext>(), find: row.find,
      density: "balanced" as const, densityEpoch: 0 };
    expect(sameWorkRowProps(run, { ...run })).toBe(true);
    expect(sameWorkRowProps(run, { ...run, extra: 1 } as typeof run)).toBe(false);
  });

  it("lets a folded run ignore every context field its rows never read", () => {
    const all = { ...baseProps, keyedApprovalId: null as string | null,
      checkingApprovalIds: new Set<string>(), pathRoot: "/repo", sessionKey: "claude#a" };
    const find = { active: false, matches: new Set<string>(), currentKey: null, register: () => undefined };
    const run = { run: {}, all, find, density: "balanced" as const, densityEpoch: 0 };
    // Approvals, focus, restore points and the subagent selection are other rows' business.
    expect(sameWorkRowProps(run, { ...run, all: { ...all, approvals: { x: { verdict: "allow" as const } },
      focused: true, activeApprovalIds: new Set(["a"]), keyedApprovalId: "a",
      turnRestorePoints: new Map(), selectedSubagentKey: "s" } })).toBe(true);
    expect(sameWorkRowProps(run, { ...run, all: { ...all, pathRoot: "/other" } })).toBe(false);
    expect(sameWorkRowProps(run, { ...run, all: { ...all, sessionKey: "claude#b" } })).toBe(false);
    expect(sameWorkRowProps(run, { ...run, run: {} })).toBe(false);
    expect(sameWorkRowProps(run, { ...run, find: { ...find } })).toBe(false);
    expect(sameWorkRowProps(run, { ...run, densityEpoch: 1 })).toBe(false);
    // A grid's tiles also draw the selection.
    expect(sameSubagentGridProps(run, { ...run, all: { ...all, focused: true } })).toBe(true);
    expect(sameSubagentGridProps(run, { ...run, all: { ...all, selectedSubagentKey: "s" } })).toBe(false);
    expect(sameSubagentGridProps(run, { ...run, all: { ...all, onSelectSubagent: () => undefined } }))
      .toBe(false);
  });

  it("announces a finished turn by its newest message, on one line", () => {
    const text = (id: string, body: string): ConvItem =>
      ({ kind: "text", id, text: body, streaming: false });
    expect(finishedTurnAnnouncement([])).toBe("Agent response finished.");
    expect(finishedTurnAnnouncement([text("a", "  \n ")])).toBe("Agent response finished.");
    expect(finishedTurnAnnouncement([
      text("a", "older"), text("b", "Done:\n\n  tests   pass. "),
      { kind: "tool", id: "t", tool: "Bash", summary: "ls", input: {} }, text("c", " "),
    ])).toBe("Agent response: Done: tests pass.");
    const long = finishedTurnAnnouncement([text("a", "x".repeat(600))]);
    expect(long).toBe(`Agent response: ${"x".repeat(497)}…`);
  });

  it("keeps layout containment out of the rows the pane measures", () => {
    // A skipped subtree lays out nothing inside itself, so content-visibility
    // on the row hands the prompt anchor, the steppers and the find scroll an
    // estimate at best and a zero rect on any descendant. Nothing in this
    // suite can prove the scroll arithmetic survives that — it has no live DOM
    // — so the row stays uncontained until someone has driven the real app.
    // Declarations, not the comment above the rule that says why there are none.
    expect(css).not.toMatch(/content-visibility\s*:/u);
    expect(css).not.toMatch(/contain-intrinsic-size\s*:/u);
    // The rects in question all come from this one registered node.
    expect(source).toContain("ref={node => props.find.register(props.findKey, node)}");
  });

  it("releases session A's queue-edit lock when switching sessionKey A -> B", () => {
    const events: Array<[string, string, boolean]> = [];
    let activeKey = "claude#a";
    const forward = createQueueEditingForwarder(
      () => {
        const boundKey = activeKey;
        return (id: string, editing: boolean) => {
          events.push([boundKey, id, editing]);
        };
      },
    );

    // Open editor on session A, then commit session B in useLayoutEffect before
    // SteeringQueue's unmount useEffect cleanup runs.
    forward("queued-1", true);
    activeKey = "claude#b";
    forward("queued-1", false);

    expect(events).toEqual([
      ["claude#a", "queued-1", true],
      ["claude#a", "queued-1", false],
    ]);
    expect(transcriptSource).toContain("key={props.sessionKey ?? \"\"}");
    expect(transcriptSource).toContain("sessionKey={props.sessionKey}");
  });
});
